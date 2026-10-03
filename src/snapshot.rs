//! Bounded, immutable, read-only semantic snapshot of one accessibility root.
//!
//! This is the adapter-side consumer shape from the accepted contract
//! (`bitty-terminal-docs/specifications/accessibility-extraction-contract.md`):
//! a derived projection that is never authority. Building or updating a
//! snapshot cannot mutate grid, cursor, modes, scrollback, attachment,
//! focus, or policy: the builder takes owned values from the host and the
//! finished [`Snapshot`] exposes only shared references. Nothing is
//! persisted and nothing is published on any event bus.
//!
//! Every bound below is finite and fail-closed; silent truncation is
//! non-conforming because it would misname a surface to assistive
//! technology. The ceilings are the accepted adapter ceilings fixed by
//! this implementation: they bound untrusted input (window titles, grid
//! text, chrome labels) to a finite allocation.

use std::fmt;

use crate::A11yError;
use crate::handles::{Anchor, ElementHandle, Generation, HandleMap};

/// Accepted ceiling in characters for an accessible name, label, or
/// announced state.
///
/// Longer input is rejected with [`A11yError::NameTooLong`], never
/// silently truncated.
pub const MAX_ACCESSIBLE_NAME_LEN: usize = 256;

/// Accepted ceiling on nodes in one snapshot.
///
/// A grid contributes at most its row count plus structural nodes, so a
/// real grid lands far below this cap; only adversarial or
/// programming-error input trips it, fail-closed via
/// [`A11yError::TooManyNodes`]. Row content counts toward the node cap
/// (one node per non-blank row) but is never subject to the name cap.
pub const MAX_SNAPSHOT_NODES: usize = 4096;

/// Candidate accessible roles: the fixed v1 vocabulary.
///
/// Whether role names follow a platform convention by name or through an
/// explicit mapping table was parked to this implementation by the
/// contract: this crate fixes the spelling below as the v1 vocabulary
/// and each platform backend maps it to the native model. Terminal text
/// runs expose text only, never styling attributes (the fidelity
/// boundary).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Running text (terminal leaf and terminal rows).
    Text,
    /// Grouping container (root, row, column, bordered block).
    Group,
    /// Code block.
    Code,
    /// Table.
    Table,
    /// List.
    List,
    /// Activatable button (declared purpose `button`).
    Button,
    /// Text input (declared purpose `input`).
    Input,
    /// Separator (horizontal rule).
    Separator,
    /// Image placement. Exposure names the boundary: no pixel
    /// description is promised.
    Image,
}

impl Role {
    /// Fixed v1 vocabulary spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Group => "group",
            Self::Code => "code",
            Self::Table => "table",
            Self::List => "list",
            Self::Button => "button",
            Self::Input => "input",
            Self::Separator => "separator",
            Self::Image => "image",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Presentable scene node kinds consumed from the host scene model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneKind {
    /// Styled span.
    Text,
    /// Horizontal row of children.
    Row,
    /// Vertical column of children.
    Column,
    /// Bordered block with a single child.
    Block,
    /// Image placement reference.
    Image,
    /// Code block with language hint.
    CodeBlock,
    /// Headless bounded table model.
    Table,
    /// Headless bounded list model.
    List,
    /// Horizontal rule.
    Rule,
    /// Forward-compatible fallback for kinds from a newer producer.
    /// Never mapped: it fails closed at validation.
    Unknown,
}

/// Maps a scene kind to its accessible role.
///
/// # Errors
///
/// Returns [`A11yError::UnmappedSceneKind`] for [`SceneKind::Unknown`];
/// the whole snapshot then fails closed rather than omitting the node
/// silently.
pub fn role_of(kind: SceneKind) -> Result<Role, A11yError> {
    match kind {
        SceneKind::Text => Ok(Role::Text),
        SceneKind::Row | SceneKind::Column | SceneKind::Block => Ok(Role::Group),
        SceneKind::CodeBlock => Ok(Role::Code),
        SceneKind::Table => Ok(Role::Table),
        SceneKind::List => Ok(Role::List),
        SceneKind::Rule => Ok(Role::Separator),
        SceneKind::Image => Ok(Role::Image),
        SceneKind::Unknown => Err(A11yError::UnmappedSceneKind),
    }
}

/// Declared purpose of an interactive node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InteractivePurpose {
    /// Activatable button.
    Button,
    /// Text input.
    Input,
}

impl InteractivePurpose {
    /// Declared-purpose spelling consumed from the host.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::Input => "input",
        }
    }

    /// Resolves a declared purpose to its role.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::UnmappedSceneKind`] for any purpose other
    /// than `button` or `input`, so a future purpose cannot present
    /// under a guessed role.
    pub fn parse(purpose: &str) -> Result<Self, A11yError> {
        match purpose {
            "button" => Ok(Self::Button),
            "input" => Ok(Self::Input),
            _ => Err(A11yError::UnmappedSceneKind),
        }
    }

    /// Role for this purpose.
    #[must_use]
    pub const fn role(self) -> Role {
        match self {
            Self::Button => Role::Button,
            Self::Input => Role::Input,
        }
    }
}

/// Chrome surface kinds with accessibility exposure.
///
/// Read-only structure with state announced; never a tab stop and never
/// a focus target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChromeKind {
    /// Status bar.
    Bar,
    /// Side rail.
    Rail,
    /// Tab strip.
    TabStrip,
    /// Notification surface (announces on transition, never captures
    /// focus).
    Notification,
    /// Ephemeral overlay surface; confines focus while modal.
    Overlay,
}

impl ChromeKind {
    /// Fixed v1 spelling for this chrome kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Rail => "rail",
            Self::TabStrip => "tablist",
            Self::Notification => "notification",
            Self::Overlay => "overlay",
        }
    }
}

impl fmt::Display for ChromeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Live cursor position on the active grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CursorPosition {
    /// Zero-based grid row.
    pub row: u32,
    /// Zero-based grid column.
    pub col: u32,
}

/// One contiguous span of readable terminal cell content.
///
/// Row content is content, not an accessible name: it is never subject
/// to the name cap and stays bounded by the grid itself. Styling is
/// deliberately absent (the fidelity boundary: readable text runs and
/// cursor position only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRun {
    /// Zero-based grid row this run was read from.
    pub row: u32,
    /// Row content with trailing blanks trimmed.
    pub text: String,
}

/// What one snapshot node exposes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum NodePayload {
    Root {
        name: String,
    },
    TerminalLeaf {
        title: String,
        cursor: CursorPosition,
        cursor_visible: bool,
    },
    TerminalRow {
        text: String,
    },
    Chrome {
        kind: ChromeKind,
        name: String,
        state: Option<String>,
    },
    Scene {
        kind: SceneKind,
        role: Role,
    },
    Interactive {
        purpose: InteractivePurpose,
        name: String,
        enabled: bool,
    },
}

/// One node of the snapshot: identity anchor plus read-only payload.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BuiltNode {
    anchor: Anchor,
    payload: NodePayload,
}

/// Whether focus is confined to a modal overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusScope {
    /// No modal overlay is active; the full tree participates.
    Full,
    /// A modal overlay is active: focus is confined to `overlay` and
    /// underlying content is excluded while the modal is active.
    OverlayOnly {
        /// Handle of the confining overlay surface.
        overlay: ElementHandle,
    },
}

/// Builds [`Snapshot`] values, fail-closed.
///
/// Validation runs before anything is exposed: overlong names, unmapped
/// scene kinds, node-cap overflow, and structural violations abort
/// [`SnapshotBuilder::finish`] with a typed error and no partial
/// snapshot escapes, because the snapshot only exists after `finish`
/// returns [`Ok`].
pub struct SnapshotBuilder {
    generation: Generation,
    root_name: Option<String>,
    terminal: Option<(String, Vec<TextRun>, CursorPosition, bool)>,
    chrome: Vec<(ChromeKind, String, Option<String>)>,
    scene: Vec<(SceneKind, u64)>,
    interactive: Vec<(InteractivePurpose, String, bool)>,
    modal_overlay: bool,
}

impl SnapshotBuilder {
    /// Starts a build for `generation` (normally the live generation
    /// advanced by one structural change).
    #[must_use]
    pub fn new(generation: Generation) -> Self {
        Self {
            generation,
            root_name: None,
            terminal: None,
            chrome: Vec::new(),
            scene: Vec::new(),
            interactive: Vec::new(),
            modal_overlay: false,
        }
    }

    /// Names the window-scoped root (role `group`).
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::NameTooLong`] when `name` exceeds
    /// [`MAX_ACCESSIBLE_NAME_LEN`] characters.
    pub fn set_root_name(&mut self, name: &str) -> Result<(), A11yError> {
        check_name_len(name)?;
        self.root_name = Some(name.to_string());
        Ok(())
    }

    /// Sets the terminal leaf: title, readable text runs in row order,
    /// and live cursor position and visibility.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::NameTooLong`] when `title` exceeds
    /// [`MAX_ACCESSIBLE_NAME_LEN`] characters. Run text is content,
    /// not a name, and is never length-checked.
    pub fn set_terminal(
        &mut self,
        title: &str,
        runs: Vec<TextRun>,
        cursor: CursorPosition,
        cursor_visible: bool,
    ) -> Result<(), A11yError> {
        check_name_len(title)?;
        self.terminal = Some((title.to_string(), runs, cursor, cursor_visible));
        Ok(())
    }

    /// Adds a chrome surface with its accessible name and optional
    /// announced state.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::NameTooLong`] for overlong `name` or
    /// `state`.
    pub fn add_chrome(
        &mut self,
        kind: ChromeKind,
        name: &str,
        state: Option<&str>,
    ) -> Result<(), A11yError> {
        check_name_len(name)?;
        if let Some(active) = state {
            check_name_len(active)?;
        }
        self.chrome
            .push((kind, name.to_string(), state.map(str::to_string)));
        Ok(())
    }

    /// Adds a scene node by kind with its producer-assigned identity.
    ///
    /// The kind is validated at [`Self::finish`]; an unmapped kind
    /// fails the whole build.
    pub fn add_scene(&mut self, kind: SceneKind, producer_identity: u64) {
        self.scene.push((kind, producer_identity));
    }

    /// Adds an interactive node with its accessible name and enabled
    /// state.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::NameTooLong`] for an overlong `name`.
    pub fn add_interactive(
        &mut self,
        purpose: InteractivePurpose,
        name: &str,
        enabled: bool,
    ) -> Result<(), A11yError> {
        check_name_len(name)?;
        self.interactive.push((purpose, name.to_string(), enabled));
        Ok(())
    }

    /// Declares a modal overlay: while set, focus is confined to the
    /// overlay surface and underlying content is excluded.
    ///
    /// [`Self::finish`] fails closed with
    /// [`A11yError::InvalidStructure`] when no overlay surface was
    /// added.
    pub fn set_modal_overlay(&mut self, active: bool) {
        self.modal_overlay = active;
    }

    /// Validates structure, role mapping, and the node cap before
    /// anything attaches.
    fn validated_parts(self) -> Result<ValidatedParts, A11yError> {
        let root_name = self.root_name.ok_or(A11yError::InvalidStructure)?;
        let (title, runs, cursor, cursor_visible) =
            self.terminal.ok_or(A11yError::InvalidStructure)?;
        if self.modal_overlay
            && !self
                .chrome
                .iter()
                .any(|(kind, _, _)| *kind == ChromeKind::Overlay)
        {
            return Err(A11yError::InvalidStructure);
        }
        for (kind, _) in &self.scene {
            role_of(*kind)?;
        }

        let node_count: usize = 2usize
            .saturating_add(runs.len())
            .saturating_add(self.chrome.len())
            .saturating_add(self.scene.len())
            .saturating_add(self.interactive.len());
        if node_count > MAX_SNAPSHOT_NODES {
            return Err(A11yError::TooManyNodes {
                count: node_count,
                cap: MAX_SNAPSHOT_NODES,
            });
        }

        Ok(ValidatedParts {
            root_name,
            title,
            runs,
            cursor,
            cursor_visible,
            chrome: self.chrome,
            scene: self.scene,
            interactive: self.interactive,
            node_count,
        })
    }

    /// Freezes the build into an immutable [`Snapshot`] plus its
    /// generation-paired [`HandleMap`].
    ///
    /// Node order is the documented tree order: root, terminal leaf,
    /// terminal rows in row order, chrome in caller order, scene in
    /// caller order, interactive in caller order.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::InvalidStructure`] when root or terminal
    /// data is missing or a modal overlay lacks its surface,
    /// [`A11yError::UnmappedSceneKind`] for the first unmapped scene
    /// kind (validated before anything attaches), and
    /// [`A11yError::TooManyNodes`] past [`MAX_SNAPSHOT_NODES`].
    pub fn finish(self) -> Result<(Snapshot, HandleMap), A11yError> {
        let generation = self.generation;
        let modal = self.modal_overlay;
        let parts = self.validated_parts()?;
        let nodes = assemble_nodes(parts)?;

        let mut map = HandleMap::new(generation);
        let mut handles = Vec::with_capacity(nodes.len());
        for node in &nodes {
            handles.push(map.issue(node.anchor.clone())?);
        }

        let modal_handle = if modal {
            nodes
                .iter()
                .position(|node| {
                    matches!(
                        node.payload,
                        NodePayload::Chrome {
                            kind: ChromeKind::Overlay,
                            ..
                        }
                    )
                })
                .map(|index| handles[index])
        } else {
            None
        };

        Ok((
            Snapshot {
                generation,
                nodes,
                handles,
                modal_overlay: modal_handle,
            },
            map,
        ))
    }
}

fn check_name_len(name: &str) -> Result<(), A11yError> {
    let len = name.chars().count();
    if len > MAX_ACCESSIBLE_NAME_LEN {
        return Err(A11yError::NameTooLong {
            len,
            cap: MAX_ACCESSIBLE_NAME_LEN,
        });
    }
    Ok(())
}

/// Builder input after validation: every kind mapped, every bound held.
struct ValidatedParts {
    root_name: String,
    title: String,
    runs: Vec<TextRun>,
    cursor: CursorPosition,
    cursor_visible: bool,
    chrome: Vec<(ChromeKind, String, Option<String>)>,
    scene: Vec<(SceneKind, u64)>,
    interactive: Vec<(InteractivePurpose, String, bool)>,
    node_count: usize,
}

/// Assembles validated parts into nodes in documented tree order.
fn assemble_nodes(parts: ValidatedParts) -> Result<Vec<BuiltNode>, A11yError> {
    let mut nodes = Vec::with_capacity(parts.node_count);
    nodes.push(BuiltNode {
        anchor: Anchor::Root,
        payload: NodePayload::Root {
            name: parts.root_name,
        },
    });
    nodes.push(BuiltNode {
        anchor: Anchor::TerminalLeaf,
        payload: NodePayload::TerminalLeaf {
            title: parts.title,
            cursor: parts.cursor,
            cursor_visible: parts.cursor_visible,
        },
    });
    for run in parts.runs {
        let row = run.row;
        nodes.push(BuiltNode {
            anchor: Anchor::TerminalRow { row },
            payload: NodePayload::TerminalRow { text: run.text },
        });
    }
    for (kind, name, state) in parts.chrome {
        nodes.push(BuiltNode {
            anchor: Anchor::Chrome {
                kind: kind.as_str(),
            },
            payload: NodePayload::Chrome { kind, name, state },
        });
    }
    for (kind, identity) in parts.scene {
        let role = role_of(kind)?;
        nodes.push(BuiltNode {
            anchor: Anchor::Scene { identity },
            payload: NodePayload::Scene { kind, role },
        });
    }
    for (purpose, name, enabled) in parts.interactive {
        nodes.push(BuiltNode {
            anchor: Anchor::Interactive {
                purpose: purpose.as_str(),
                name: name.clone(),
            },
            payload: NodePayload::Interactive {
                purpose,
                name,
                enabled,
            },
        });
    }
    Ok(nodes)
}

/// Read-only view of one snapshot node, resolved through a handle.
#[derive(Clone, Copy, Debug)]
pub enum NodeView<'a> {
    /// Window root: role `group`, named by the window or icon title.
    Root {
        /// Accessible name.
        name: &'a str,
    },
    /// Terminal leaf: role `text`, named by the title, with live
    /// cursor position and visibility.
    Terminal {
        /// Region name (window/icon title).
        title: &'a str,
        /// Live cursor position.
        cursor: CursorPosition,
        /// Whether the cursor is rendered.
        cursor_visible: bool,
    },
    /// Terminal row: row content as text content, not an accessible
    /// name.
    TerminalRow {
        /// Row content.
        text: &'a str,
    },
    /// Chrome surface: read-only structure with announced state; no
    /// role and never a tab stop.
    Chrome {
        /// Surface kind.
        kind: ChromeKind,
        /// Accessible name.
        name: &'a str,
        /// Announced state, if any.
        state: Option<&'a str>,
    },
    /// Scene node with its mapped role.
    Scene {
        /// Producer scene kind.
        kind: SceneKind,
        /// Mapped accessible role.
        role: Role,
    },
    /// Interactive node with its resolved role and enabled state.
    Interactive {
        /// Declared purpose.
        purpose: InteractivePurpose,
        /// Accessible name.
        name: &'a str,
        /// Whether the control can be activated.
        enabled: bool,
    },
}

impl<'a> NodeView<'a> {
    /// Accessible role, when the node carries one.
    ///
    /// Chrome surfaces have no [`Role`]: they are identified by
    /// [`ChromeKind`] instead, so this returns [`None`] for them
    /// rather than inventing a mapping.
    #[must_use]
    pub const fn role(self) -> Option<Role> {
        match self {
            Self::Root { .. } => Some(Role::Group),
            Self::Terminal { .. } | Self::TerminalRow { .. } => Some(Role::Text),
            Self::Chrome { .. } => None,
            Self::Scene { role, .. } => Some(role),
            Self::Interactive { purpose, .. } => Some(purpose.role()),
        }
    }

    /// Accessible name, when the node carries one.
    ///
    /// Terminal rows carry content instead of a name; scene nodes
    /// carry their role spelling.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        match self {
            Self::Root { name } | Self::Chrome { name, .. } | Self::Interactive { name, .. } => {
                Some(name)
            }
            Self::Terminal { title, .. } => Some(title),
            Self::TerminalRow { .. } | Self::Scene { .. } => None,
        }
    }
}

/// Bounded, immutable, read-only projection of one accessibility root.
///
/// Derived, never authority: the snapshot is computed from host values,
/// is never consulted for routing or dispatch, and can be regenerated
/// from its sources at any generation. The adapter swaps to a new
/// generation atomically and never presents a partially built tree.
#[derive(Clone, Debug)]
pub struct Snapshot {
    generation: Generation,
    nodes: Vec<BuiltNode>,
    handles: Vec<ElementHandle>,
    modal_overlay: Option<ElementHandle>,
}

impl Snapshot {
    /// Projection generation this snapshot belongs to.
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    /// Node count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the snapshot holds no nodes (never true for built
    /// snapshots: root and terminal leaf always exist).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Root handle.
    #[must_use]
    pub fn root(&self) -> ElementHandle {
        self.handles[0]
    }

    /// Resolves a handle to its read-only view.
    ///
    /// Returns [`None`] for forged, foreign, or out-of-range handles;
    /// effectful paths must additionally check the generation through
    /// the [`HandleMap`].
    #[must_use]
    pub fn get(&self, handle: ElementHandle) -> Option<NodeView<'_>> {
        let (node, _) = self
            .nodes
            .iter()
            .zip(self.handles.iter())
            .find(|(_, issued)| **issued == handle)?;
        Some(view_of(node))
    }

    /// Handles in documented tree order: root, terminal leaf,
    /// terminal rows in row order, chrome in caller order, scene in
    /// caller order, interactive in caller order.
    pub fn preorder(&self) -> impl Iterator<Item = ElementHandle> + '_ {
        self.handles.iter().copied()
    }

    /// Assistive tab order over exposed elements.
    ///
    /// Filter-based: chrome surfaces are never tab stops, so only the
    /// terminal leaf and interactive nodes participate. The exclusion
    /// is enforced in this one place.
    #[must_use]
    pub fn tab_order(&self) -> Vec<ElementHandle> {
        self.nodes
            .iter()
            .zip(self.handles.iter())
            .filter(|(node, _)| {
                matches!(
                    node.payload,
                    NodePayload::TerminalLeaf { .. } | NodePayload::Interactive { .. }
                )
            })
            .map(|(_, handle)| *handle)
            .collect()
    }

    /// Whether focus is confined to a modal overlay.
    #[must_use]
    pub const fn focus_scope(&self) -> FocusScope {
        match self.modal_overlay {
            Some(overlay) => FocusScope::OverlayOnly { overlay },
            None => FocusScope::Full,
        }
    }

    /// Resolves an accessible name or title to its handle.
    ///
    /// Matches root name, terminal title, chrome names, and
    /// interactive names in tree order. Returns [`None`] for an
    /// unknown label: label resolution fails closed and never guesses.
    /// Terminal row content and scene nodes are not labels and never
    /// match.
    #[must_use]
    pub fn lookup_label(&self, label: &str) -> Option<ElementHandle> {
        self.nodes
            .iter()
            .zip(self.handles.iter())
            .find(|(node, _)| match &node.payload {
                NodePayload::Root { name }
                | NodePayload::Chrome { name, .. }
                | NodePayload::Interactive { name, .. } => name.as_str() == label,
                NodePayload::TerminalLeaf { title, .. } => title.as_str() == label,
                NodePayload::TerminalRow { .. } | NodePayload::Scene { .. } => false,
            })
            .map(|(_, handle)| *handle)
    }
}

fn view_of(node: &BuiltNode) -> NodeView<'_> {
    match &node.payload {
        NodePayload::Root { name } => NodeView::Root { name },
        NodePayload::TerminalLeaf {
            title,
            cursor,
            cursor_visible,
        } => NodeView::Terminal {
            title,
            cursor: *cursor,
            cursor_visible: *cursor_visible,
        },
        NodePayload::TerminalRow { text } => NodeView::TerminalRow { text },
        NodePayload::Chrome { kind, name, state } => NodeView::Chrome {
            kind: *kind,
            name,
            state: state.as_deref(),
        },
        NodePayload::Scene { kind, role } => NodeView::Scene {
            kind: *kind,
            role: *role,
        },
        NodePayload::Interactive {
            purpose,
            name,
            enabled,
        } => NodeView::Interactive {
            purpose: *purpose,
            name,
            enabled: *enabled,
        },
    }
}
