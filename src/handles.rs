//! Generation-paired, unforgeable element handles.
//!
//! Handles are snapshot-scoped, opaque, and ephemeral: they carry no raw
//! UI, compositor, or memory pointer and never alias a `PanelId`,
//! `ViewId`, or `TerminalId`. Every handle is paired with the projection
//! generation it was derived from; a structural change advances the
//! generation and invalidates prior-generation handles by construction.
//! Handles are not persisted and are not transferable between windows,
//! sessions, or adapter generations.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::A11yError;

/// Monotonic revision of the derived snapshot.
///
/// Advances only on a structural change, never on an identical rebuild.
/// The adapter treats any handle or snapshot from an older generation as
/// stale and fails closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    /// The first generation of a fresh adapter.
    #[must_use]
    pub const fn initial() -> Self {
        Self(0)
    }

    /// The numeric revision, for diagnostics and backend reconciliation.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Advances to the next generation after a structural change.
    ///
    /// Saturates at [`u64::MAX`] rather than wrapping: a wrapped
    /// generation could alias a live one, so saturation fails safe
    /// (the adapter pins at the ceiling and keeps rejecting older
    /// generations).
    #[must_use]
    pub const fn advance(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "generation {}", self.0)
    }
}

/// Cross-snapshot identity anchor for one exposed element.
///
/// Reconciliation is by identity, never by position: terminal rows key on
/// the terminal leaf plus row ordinal, chrome nodes key on their kind,
/// scene nodes key on their producer-assigned identity, and interactive
/// nodes key on declared purpose plus accessible name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Anchor {
    /// The window-scoped root.
    Root,
    /// The terminal leaf (role `text` region).
    TerminalLeaf,
    /// One terminal row: content keyed by row ordinal, never an
    /// accessible name.
    TerminalRow {
        /// Zero-based grid row.
        row: u32,
    },
    /// A chrome surface keyed by kind.
    Chrome {
        /// Surface kind spelling (`bar`, `rail`, `tablist`,
        /// `notification`, `overlay`).
        kind: &'static str,
    },
    /// A scene node keyed by its producer-assigned identity.
    Scene {
        /// Opaque producer-assigned identity; never a memory pointer.
        identity: u64,
    },
    /// An interactive node keyed by declared purpose plus name.
    Interactive {
        /// Declared purpose (`button` or `input`).
        purpose: &'static str,
        /// Accessible name at build time.
        name: String,
    },
}

/// Whether an anchor may ever own focus.
///
/// Chrome surfaces are excluded by default: they announce state and
/// expose read-only structure but are never focus targets. Scene and
/// structural nodes are not focus owners either; only the terminal leaf
/// (and its rows) and enabled interactive nodes participate in focus
/// association.
impl Anchor {
    #[must_use]
    pub(crate) const fn is_focusable(&self) -> bool {
        match self {
            Self::TerminalLeaf | Self::TerminalRow { .. } | Self::Interactive { .. } => true,
            Self::Root | Self::Chrome { .. } | Self::Scene { .. } => false,
        }
    }
}

/// Opaque element handle, scoped to the [`HandleMap`] that issued it.
///
/// There is no public constructor: handles cannot be forged. Presenting
/// a handle from another map (or an out-of-range index) fails with
/// [`A11yError::UnknownHandle`]; presenting a handle from an older
/// generation fails with [`A11yError::StaleGeneration`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElementHandle {
    epoch: u64,
    index: u32,
}

impl ElementHandle {
    /// Numeric index, stable for the lifetime of the owning snapshot.
    #[must_use]
    pub const fn index(self) -> usize {
        self.index as usize
    }
}

/// Issues once per [`HandleMap`] so handles from different maps never
/// alias, even across processes restarts within one host lifetime.
static MAP_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Generation-paired registry binding handles to anchors.
///
/// One map exists per snapshot generation. Resolution always takes the
/// live generation and fails closed: foreign or out-of-range handles
/// yield [`A11yError::UnknownHandle`], and handles from a superseded
/// generation yield [`A11yError::StaleGeneration`].
#[derive(Clone, Debug)]
pub struct HandleMap {
    epoch: u64,
    generation: Generation,
    anchors: Vec<Anchor>,
}

impl HandleMap {
    /// Starts an empty map for `generation`.
    ///
    /// The epoch is drawn from a process-wide counter, so two maps never
    /// share an epoch and cross-map handles are always foreign.
    #[must_use]
    pub fn new(generation: Generation) -> Self {
        Self {
            epoch: MAP_EPOCH.fetch_add(1, Ordering::Relaxed),
            generation,
            anchors: Vec::new(),
        }
    }

    /// Issues a handle for `anchor` in this map's generation.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::TooManyNodes`] when the handle index
    /// would overflow [`u32`]; the caller ([`crate::SnapshotBuilder`])
    /// caps the node count far below that, so this is a second fence,
    /// not a reachable path.
    pub(crate) fn issue(&mut self, anchor: Anchor) -> Result<ElementHandle, A11yError> {
        let index = u32::try_from(self.anchors.len()).map_err(|_| A11yError::TooManyNodes {
            count: self.anchors.len().saturating_add(1),
            cap: u32::MAX as usize,
        })?;
        self.anchors.push(anchor);
        Ok(ElementHandle {
            epoch: self.epoch,
            index,
        })
    }

    /// Generation this map belongs to.
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    /// Handle count in this map.
    #[must_use]
    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    /// Whether the map holds no handles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    /// Resolves `handle` against the live generation.
    ///
    /// # Errors
    ///
    /// Returns [`A11yError::UnknownHandle`] for a forged, foreign, or
    /// out-of-range handle, and [`A11yError::StaleGeneration`] when the
    /// handle belongs to a superseded generation.
    pub fn resolve(&self, handle: ElementHandle, live: Generation) -> Result<&Anchor, A11yError> {
        if handle.epoch != self.epoch {
            return Err(A11yError::UnknownHandle);
        }
        let anchor = self
            .anchors
            .get(handle.index())
            .ok_or(A11yError::UnknownHandle)?;
        if self.generation != live {
            return Err(A11yError::StaleGeneration {
                presented: self.generation.as_u64(),
                live: live.as_u64(),
            });
        }
        Ok(anchor)
    }

    /// Anchor for `handle` without a generation check.
    ///
    /// Intended for diagnostics only; every effectful path must use
    /// [`Self::resolve`].
    #[must_use]
    pub fn anchor(&self, handle: ElementHandle) -> Option<&Anchor> {
        if handle.epoch != self.epoch {
            return None;
        }
        self.anchors.get(handle.index())
    }
}
