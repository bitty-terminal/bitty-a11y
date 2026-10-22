//! Component server: owns the accessibility projection and serves bounded
//! snapshots and diffs over the v1 stdio protocol.
//!
//! The binary entry point (`bitty-a11y` with no arguments) calls
//! [`serve_stdio`]. Tests call [`serve`] with explicit streams, or drive
//! [`Session`] directly with an injected clock.
//!
//! # What the server is
//!
//! Core owns terminal truth and the projection decision; this process owns
//! the accessibility side of the split: it validates every publication with
//! the same [`SnapshotBuilder`] and [`Adapter`] path as the linked crate,
//! assigns a stable id per projection anchor, and answers with canonical tree
//! rows. It is deliberately a thin, policy-free adapter:
//!
//! - It carries no installer, download source, capability policy, consent,
//!   safe-mode decision, platform bridge, or filesystem/network access.
//! - Every publication must arrive through [`Message::PublishBegin`] /
//!   [`Message::PublishNode`] / [`Message::PublishCommit`]; the node stream is
//!   checked against the declared counts, so no partial projection is ever
//!   swapped in.
//! - A generation must advance; a stale generation is refused and nothing
//!   swaps.
//! - Rows are chunked so one frame stays under
//!   [`MAX_FRAME_BYTES`](super::protocol::MAX_FRAME_BYTES).
//!
//! # Stable ids and stale-tree behavior
//!
//! An element id is assigned to a projection [`Anchor`] the first time it is
//! seen and retained while that anchor survives. It is never reused: when an
//! anchor disappears its id is retired, and a re-added anchor receives a new
//! id. A host that applies a diff at generation `G` and then asks for a diff
//! `since` an older or unknown generation receives a **full snapshot** rather
//! than a fabricated delta. A diff or ack for a generation the server has not
//! published fails with `stale_generation`.
//!
//! # Safe mode and deprivation
//!
//! `Hello.mode` declares safe mode and whether the host granted the exposure
//! capability. Safe mode, a withheld grant, or both make the session
//! deprived: the handshake completes with zero capabilities, every projection
//! exchange fails closed with `denied`, no tree is staged or swapped, and
//! `Shutdown` still works. The component never self-grants.
//!
//! # Shutdown, EOF, and crashes
//!
//! `Shutdown` (or stdin EOF) writes `ShutdownAck` for an explicit shutdown
//! and exits 0. A framing or decode violation writes a connection-level
//! `Error` and exits 3. A component crash cannot be cleaned from inside; the
//! Core host client must place it in its own process group and treat EOF as
//! `component_lost` (documented remaining host work). The server starts no
//! threads, so it cannot leave background work behind.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};
use std::time::Instant;

use crate::{
    A11yError, Adapter, AnnouncementKind, ChromeKind, CursorPosition, FocusOwner, Generation,
    HandleMap, InteractivePurpose, MAX_SNAPSHOT_NODES, NodeView, SceneKind, Snapshot,
    SnapshotBuilder, TextRun, handles::ElementHandle,
};

use super::descriptor::COMPONENT_NAME;
use super::protocol::{
    BOUNDS_UNIT_CHARS, Bounds, CAPABILITY_MASK, CONNECTION_ID, ErrorKind, FocusCell, FrameReader,
    HELLO_CAPABILITY_GRANTED, HELLO_MODE_MASK, HELLO_SAFE_MODE, MAX_DIFF_PAGE_REMOVED,
    MAX_DIFF_PAGE_ROWS, MAX_PUBLISH_TEXT_BYTES, MAX_SNAPSHOT_PAGE_ROWS, MAX_TREE_DEPTH, Message,
    PublishNode, ROLE_GROUP, ROLE_NONE, ROLE_TEXT, STATE_CURSOR_VISIBLE, STATE_ENABLED,
    STATE_FOCUSABLE, STATE_FOCUSED, STATE_MODAL, STATE_SELECTED, SURFACE_NONE, Selection,
    SelectionSpan, TreeRow, chrome_code, decode_message, page_count, role_code, write_frame,
};

/// Version reported in `HelloAck` and by `--version` (the crate version).
pub use super::COMPONENT_VERSION;

/// Clean shutdown (explicit `Shutdown` or stdin EOF).
pub const SERVER_EXIT_OK: u8 = 0;
/// Usage refusal (the operator asked for something the binary does not
/// serve); mirrors the other component binaries.
pub const SERVER_EXIT_REFUSED: u8 = 2;
/// Handshake or protocol violation; the peer is told why first when the
/// stream still permits a write.
pub const SERVER_EXIT_PROTOCOL: u8 = 3;
/// The server could not start or write its output stream.
pub const SERVER_EXIT_INTERNAL: u8 = 4;

/// Requests allowed in one burst.
pub const RATE_CAPACITY: u32 = 120;

/// Sustained request rate per second.
pub const RATE_PER_SECOND: u32 = 60;

/// Consecutive rate violations before the connection is closed.
pub const RATE_MAX_VIOLATIONS: u32 = 16;

const ERR_HANDSHAKE_REQUIRED: &str = "handshake required";
const ERR_PROTOCOL_VERSION: &str = "protocol version unsupported";
const ERR_COMPONENT_MISMATCH: &str = "component name mismatch";
const ERR_UNKNOWN_HELLO_MODE: &str = "unknown hello mode";
const ERR_DUPLICATE_HANDSHAKE: &str = "duplicate handshake";
const ERR_DENIED: &str = "accessibility exposure denied by host policy";
const ERR_RATE_LIMITED: &str = "request rate exceeded";
const ERR_NO_PUBLISH: &str = "no publication in progress";
const ERR_PUBLISH_REQUEST: &str = "publication request mismatch";
const ERR_PUBLISH_COUNT: &str = "publication entry count mismatch";
const ERR_PUBLISH_ORDER: &str = "duplicate or overflowing publication entry";
const ERR_PUBLISH_BUDGET: &str = "publication text budget exceeded";
const ERR_STALE_GENERATION: &str = "stale generation";
const ERR_UNAVAILABLE: &str = "no projection published";
const ERR_INTERNAL: &str = "internal error";
const ERR_ID_SPACE: &str = "element id space exhausted";

/// Serves the protocol on stdin/stdout.
#[must_use]
pub fn serve_stdio() -> u8 {
    serve(io::stdin(), io::stdout())
}

/// Serves the protocol on explicit streams (tests and embedders).
///
/// Returns [`SERVER_EXIT_OK`], [`SERVER_EXIT_PROTOCOL`], or
/// [`SERVER_EXIT_INTERNAL`].
pub fn serve<R, W>(input: R, output: W) -> u8
where
    R: Read,
    W: Write,
{
    let mut reader = FrameReader::new(input);
    let mut writer = io::BufWriter::new(output);
    let mut session = Session::new();
    loop {
        let payload = match reader.read_frame() {
            Ok(None) => return SERVER_EXIT_OK,
            Ok(Some(payload)) => payload,
            Err(error) => {
                let _ = write_connection_error(&mut writer, &error.to_string());
                return SERVER_EXIT_PROTOCOL;
            }
        };
        let message = match decode_message(&payload) {
            Ok(message) => message,
            Err(error) => {
                let _ = write_connection_error(&mut writer, &error.to_string());
                return SERVER_EXIT_PROTOCOL;
            }
        };
        let outcome = session.handle(message, Instant::now());
        for reply in &outcome.replies {
            if write_frame(&mut writer, reply).is_err() {
                return SERVER_EXIT_INTERNAL;
            }
        }
        if writer.flush().is_err() {
            return SERVER_EXIT_INTERNAL;
        }
        if let Some(code) = outcome.close {
            return code;
        }
    }
}

fn write_connection_error<W: Write>(writer: &mut W, detail: &str) -> Result<(), ()> {
    let message = Message::Error {
        request: CONNECTION_ID,
        kind: ErrorKind::Protocol,
        message: bounded_diagnostic(detail),
    };
    write_frame(writer, &message).map_err(|_| ())?;
    writer.flush().map_err(|_| ())
}

/// One bounded static diagnostic.
///
/// Wire and decode errors never carry peer content (strings are validated
/// before use and the formatted error is a fixed shape plus numbers), but the
/// result is still clamped so a future detail cannot grow the frame.
fn bounded_diagnostic(detail: &str) -> String {
    const FALLBACK: &str = "invalid protocol frame";
    if detail.is_empty() || detail.len() > super::protocol::MAX_ERROR_MESSAGE_BYTES {
        return FALLBACK.to_owned();
    }
    let filtered: String = detail
        .chars()
        .map(|ch| if ch.is_control() { '?' } else { ch })
        .collect();
    if filtered.is_empty() {
        FALLBACK.to_owned()
    } else {
        filtered
    }
}

/// Result of one handled message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionOutcome {
    /// Frames to write, in order.
    pub replies: Vec<Message>,
    /// Exit code to use after writing `replies`, when the session ends.
    pub close: Option<u8>,
}

/// One connection's protocol state.
///
/// The session is single-threaded and owns the live projection, the
/// acknowledged baseline used for diffs, the stable id map, and the rate
/// gate. Tests inject [`Instant`] into [`Session::handle`] so rate policy is
/// deterministic.
pub struct Session {
    phase: Phase,
    deprived: bool,
    live: Option<LiveTree>,
    baseline: Option<Baseline>,
    pending: Option<PendingPublish>,
    ids: IdMap,
    rate: RateGate,
    rate_violations: u32,
    adapter: Adapter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AwaitingHello,
    Ready,
    Closed,
}

struct LiveTree {
    generation: u64,
    rows: Vec<TreeRow>,
    focus_id: Option<u64>,
    selection: Option<Selection>,
}

struct Baseline {
    generation: u64,
    rows: BTreeMap<u64, TreeRow>,
}

struct PendingPublish {
    request: u64,
    generation: u64,
    root_name: String,
    title: String,
    cursor_row: u32,
    cursor_col: u32,
    cursor_visible: bool,
    terminal_rows: Vec<TextRun>,
    chrome: Vec<(ChromeKind, String, Option<String>)>,
    scene: Vec<(SceneKind, u64)>,
    interactive: Vec<(InteractivePurpose, String, bool)>,
    notices: Vec<(AnnouncementKind, String)>,
    counts: PublishCounts,
    seen_rows: HashSet<u32>,
    seen_chrome: HashSet<u8>,
    seen_scene: HashSet<u64>,
    seen_interactive: HashSet<(u8, String)>,
    text_bytes: usize,
    modal_overlay: bool,
    focus: Option<String>,
    selection: Option<Selection>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PublishCounts {
    terminal_rows: u32,
    chrome: u32,
    scene: u32,
    interactive: u32,
    notices: u32,
}

impl PublishCounts {
    fn of(pending: &PendingPublish) -> Self {
        Self {
            terminal_rows: u32::try_from(pending.terminal_rows.len()).unwrap_or(u32::MAX),
            chrome: u32::try_from(pending.chrome.len()).unwrap_or(u32::MAX),
            scene: u32::try_from(pending.scene.len()).unwrap_or(u32::MAX),
            interactive: u32::try_from(pending.interactive.len()).unwrap_or(u32::MAX),
            notices: u32::try_from(pending.notices.len()).unwrap_or(u32::MAX),
        }
    }
}

#[derive(Clone, Debug)]
struct IdMap {
    by_anchor: HashMap<AnchorKey, u64>,
    next: u64,
}

/// Hashable projection anchor mirroring [`crate::Anchor`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum AnchorKey {
    Root,
    TerminalLeaf,
    TerminalRow { row: u32 },
    Chrome { kind: u8 },
    Scene { identity: u64 },
    Interactive { purpose: u8, name: String },
}

impl AnchorKey {
    fn of(anchor: &crate::Anchor) -> Self {
        match anchor {
            crate::Anchor::Root => Self::Root,
            crate::Anchor::TerminalLeaf => Self::TerminalLeaf,
            crate::Anchor::TerminalRow { row } => Self::TerminalRow { row: *row },
            crate::Anchor::Chrome { kind } => {
                // The anchor stores the fixed surface spelling; map it back to
                // the numeric code so the key is stable across spellings.
                let code = match *kind {
                    "bar" => super::protocol::SURFACE_BAR,
                    "rail" => super::protocol::SURFACE_RAIL,
                    "tablist" => super::protocol::SURFACE_TABLIST,
                    "notification" => super::protocol::SURFACE_NOTIFICATION,
                    "overlay" => super::protocol::SURFACE_OVERLAY,
                    _ => 0,
                };
                Self::Chrome { kind: code }
            }
            crate::Anchor::Scene { identity } => Self::Scene {
                identity: *identity,
            },
            crate::Anchor::Interactive { purpose, name } => Self::Interactive {
                purpose: match *purpose {
                    "button" => super::protocol::purpose_code(InteractivePurpose::Button),
                    "input" => super::protocol::purpose_code(InteractivePurpose::Input),
                    _ => 0,
                },
                name: name.clone(),
            },
        }
    }
}

impl IdMap {
    fn new() -> Self {
        Self {
            by_anchor: HashMap::new(),
            next: 1,
        }
    }

    fn assign(&mut self, anchor: &crate::Anchor) -> Result<u64, Refusal> {
        let key = AnchorKey::of(anchor);
        if let Some(id) = self.by_anchor.get(&key) {
            return Ok(*id);
        }
        let id = self.next;
        if id == u64::MAX {
            return Err(Refusal {
                kind: ErrorKind::Budget,
                message: ERR_ID_SPACE,
            });
        }
        self.next = id.saturating_add(1);
        self.by_anchor.insert(key, id);
        Ok(id)
    }

    fn retain(&mut self, live: &HashSet<AnchorKey>) {
        self.by_anchor.retain(|key, _| live.contains(key));
    }
}

struct RateGate {
    tokens: u32,
    last: Option<Instant>,
}

impl RateGate {
    const fn new() -> Self {
        Self {
            tokens: RATE_CAPACITY,
            last: None,
        }
    }

    fn allow(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last {
            let elapsed = now.saturating_duration_since(last);
            let millis = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
            let refill = u32::try_from(millis.saturating_mul(u64::from(RATE_PER_SECOND)) / 1000)
                .unwrap_or(u32::MAX);
            if refill > 0 {
                self.tokens = self.tokens.saturating_add(refill).min(RATE_CAPACITY);
                self.last = Some(now);
            }
        } else {
            self.last = Some(now);
        }
        if self.tokens == 0 {
            false
        } else {
            self.tokens -= 1;
            true
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Refusal {
    kind: ErrorKind,
    message: &'static str,
}

impl Session {
    /// Starts a session awaiting `Hello`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: Phase::AwaitingHello,
            deprived: false,
            live: None,
            baseline: None,
            pending: None,
            ids: IdMap::new(),
            rate: RateGate::new(),
            rate_violations: 0,
            adapter: Adapter::new(),
        }
    }

    /// Handles one decoded message.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn handle(&mut self, message: Message, now: Instant) -> SessionOutcome {
        let mut outcome = SessionOutcome::default();
        if self.phase == Phase::Closed {
            outcome.close = Some(SERVER_EXIT_PROTOCOL);
            return outcome;
        }

        match message {
            Message::Hello {
                min,
                max,
                component,
                version: _,
                mode,
            } => {
                self.handle_hello(min, max, &component, mode, &mut outcome);
                return outcome;
            }
            Message::Shutdown => {
                outcome.replies.push(Message::ShutdownAck);
                outcome.close = Some(SERVER_EXIT_OK);
                self.phase = Phase::Closed;
                return outcome;
            }
            _ => {}
        }

        if self.phase != Phase::Ready {
            outcome.replies.push(Message::Error {
                request: CONNECTION_ID,
                kind: ErrorKind::Protocol,
                message: ERR_HANDSHAKE_REQUIRED.to_owned(),
            });
            outcome.close = Some(SERVER_EXIT_PROTOCOL);
            self.phase = Phase::Closed;
            return outcome;
        }

        if self.consumes_rate(&message) && !self.rate.allow(now) {
            self.rate_violations = self.rate_violations.saturating_add(1);
            outcome.replies.push(Message::Error {
                request: message.request_id().unwrap_or(CONNECTION_ID),
                kind: ErrorKind::RateLimited,
                message: ERR_RATE_LIMITED.to_owned(),
            });
            if self.rate_violations >= RATE_MAX_VIOLATIONS {
                outcome.close = Some(SERVER_EXIT_PROTOCOL);
                self.phase = Phase::Closed;
            }
            return outcome;
        }
        self.rate_violations = 0;
        let request_id = message.request_id().unwrap_or(CONNECTION_ID);

        match message {
            Message::PublishBegin {
                request,
                generation,
                root_name,
                title,
                cursor_row,
                cursor_col,
                cursor_visible,
                terminal_rows,
                chrome,
                scene,
                interactive,
                notices,
                modal_overlay,
                focus,
                selection,
            } => {
                if self.deprived {
                    self.pending = None;
                    outcome.replies.push(denied(request));
                } else if let Err(refusal) = self.begin_publish(
                    request,
                    generation,
                    root_name,
                    title,
                    cursor_row,
                    cursor_col,
                    cursor_visible,
                    PublishCounts {
                        terminal_rows,
                        chrome,
                        scene,
                        interactive,
                        notices,
                    },
                    modal_overlay,
                    focus,
                    selection,
                ) {
                    outcome.replies.push(refusal_message(request, refusal));
                }
            }
            Message::PublishNode { request, node } => {
                if self.deprived {
                    self.pending = None;
                    outcome.replies.push(denied(request));
                } else if let Err(refusal) = self.stage_node(request, node) {
                    self.pending = None;
                    outcome.replies.push(refusal_message(request, refusal));
                }
            }
            Message::PublishCommit {
                request,
                generation,
            } => {
                if self.deprived {
                    self.pending = None;
                    outcome.replies.push(denied(request));
                } else {
                    match self.commit_publish(request, generation) {
                        Ok(published) => outcome.replies.push(published),
                        Err(refusal) => {
                            self.pending = None;
                            outcome.replies.push(refusal_message(request, refusal));
                        }
                    }
                }
            }
            Message::SnapshotRequest { request, since } => {
                if self.deprived {
                    outcome.replies.push(denied(request));
                } else {
                    match self.snapshot_messages(request, since) {
                        Ok(replies) => outcome.replies = replies,
                        Err(refusal) => outcome.replies.push(refusal_message(request, refusal)),
                    }
                }
            }
            Message::DiffAck { generation } => {
                if self.deprived {
                    outcome.replies.push(denied(CONNECTION_ID));
                } else if let Err(refusal) = self.ack(generation) {
                    outcome
                        .replies
                        .push(refusal_message(CONNECTION_ID, refusal));
                }
            }
            Message::Hello { .. }
            | Message::HelloAck { .. }
            | Message::Shutdown
            | Message::Published { .. }
            | Message::SnapshotBegin { .. }
            | Message::SnapshotPage { .. }
            | Message::DiffBegin { .. }
            | Message::DiffPage { .. }
            | Message::Error { .. }
            | Message::ShutdownAck => {
                outcome.replies.push(Message::Error {
                    request: request_id,
                    kind: ErrorKind::Protocol,
                    message: "unexpected message".to_owned(),
                });
                outcome.close = Some(SERVER_EXIT_PROTOCOL);
                self.phase = Phase::Closed;
            }
        }
        outcome
    }

    fn handle_hello(
        &mut self,
        min: u16,
        max: u16,
        component: &str,
        mode: u8,
        outcome: &mut SessionOutcome,
    ) {
        if self.phase == Phase::Ready {
            outcome.replies.push(Message::Error {
                request: CONNECTION_ID,
                kind: ErrorKind::Protocol,
                message: ERR_DUPLICATE_HANDSHAKE.to_owned(),
            });
            outcome.close = Some(SERVER_EXIT_PROTOCOL);
            self.phase = Phase::Closed;
            return;
        }
        if mode & !HELLO_MODE_MASK != 0 {
            outcome.replies.push(Message::Error {
                request: CONNECTION_ID,
                kind: ErrorKind::Protocol,
                message: ERR_UNKNOWN_HELLO_MODE.to_owned(),
            });
            outcome.close = Some(SERVER_EXIT_PROTOCOL);
            self.phase = Phase::Closed;
            return;
        }
        if component != COMPONENT_NAME {
            outcome.replies.push(Message::Error {
                request: CONNECTION_ID,
                kind: ErrorKind::Protocol,
                message: ERR_COMPONENT_MISMATCH.to_owned(),
            });
            outcome.close = Some(SERVER_EXIT_PROTOCOL);
            self.phase = Phase::Closed;
            return;
        }
        let Some(protocol) = super::protocol::negotiate(min, max) else {
            outcome.replies.push(Message::Error {
                request: CONNECTION_ID,
                kind: ErrorKind::Protocol,
                message: ERR_PROTOCOL_VERSION.to_owned(),
            });
            outcome.close = Some(SERVER_EXIT_PROTOCOL);
            self.phase = Phase::Closed;
            return;
        };
        self.deprived = mode & HELLO_SAFE_MODE != 0 || mode & HELLO_CAPABILITY_GRANTED == 0;
        let capabilities = if self.deprived { 0 } else { CAPABILITY_MASK };
        self.phase = Phase::Ready;
        outcome.replies.push(Message::HelloAck {
            protocol,
            component: COMPONENT_NAME.to_owned(),
            version: COMPONENT_VERSION.to_owned(),
            capabilities,
        });
    }

    fn consumes_rate(&self, message: &Message) -> bool {
        match message {
            Message::Hello { .. } | Message::Shutdown => false,
            Message::PublishNode { .. } | Message::PublishCommit { .. } => self.pending.is_none(),
            _ => true,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_publish(
        &mut self,
        request: u64,
        generation: u64,
        root_name: String,
        title: String,
        cursor_row: u32,
        cursor_col: u32,
        cursor_visible: bool,
        counts: PublishCounts,
        modal_overlay: bool,
        focus: Option<String>,
        selection: Option<Selection>,
    ) -> Result<(), Refusal> {
        if let Some(live) = &self.live
            && generation <= live.generation
        {
            return Err(Refusal {
                kind: ErrorKind::StaleGeneration,
                message: ERR_STALE_GENERATION,
            });
        }
        let mut text_bytes = root_name.len().saturating_add(title.len());
        if let Some(focus) = &focus {
            text_bytes = text_bytes.saturating_add(focus.len());
        }
        if text_bytes > MAX_PUBLISH_TEXT_BYTES {
            return Err(Refusal {
                kind: ErrorKind::Budget,
                message: ERR_PUBLISH_BUDGET,
            });
        }
        self.pending = Some(PendingPublish {
            request,
            generation,
            root_name,
            title,
            cursor_row,
            cursor_col,
            cursor_visible,
            terminal_rows: Vec::new(),
            chrome: Vec::new(),
            scene: Vec::new(),
            interactive: Vec::new(),
            notices: Vec::new(),
            counts,
            seen_rows: HashSet::new(),
            seen_chrome: HashSet::new(),
            seen_scene: HashSet::new(),
            seen_interactive: HashSet::new(),
            text_bytes,
            modal_overlay,
            focus,
            selection,
        });
        Ok(())
    }

    fn stage_node(&mut self, request: u64, node: PublishNode) -> Result<(), Refusal> {
        let Some(pending) = self.pending.as_mut() else {
            return Err(Refusal {
                kind: ErrorKind::InvalidRequest,
                message: ERR_NO_PUBLISH,
            });
        };
        if pending.request != request {
            return Err(Refusal {
                kind: ErrorKind::InvalidRequest,
                message: ERR_PUBLISH_REQUEST,
            });
        }
        let add_text = |pending: &mut PendingPublish, bytes: usize| {
            pending.text_bytes = pending.text_bytes.saturating_add(bytes);
            if pending.text_bytes > MAX_PUBLISH_TEXT_BYTES {
                return Err(Refusal {
                    kind: ErrorKind::Budget,
                    message: ERR_PUBLISH_BUDGET,
                });
            }
            Ok(())
        };
        match node {
            PublishNode::TerminalRow { row, text } => {
                if pending.terminal_rows.len() >= pending.counts.terminal_rows as usize
                    || !pending.seen_rows.insert(row)
                {
                    return Err(order_refusal());
                }
                add_text(pending, text.len())?;
                pending.terminal_rows.push(TextRun { row, text });
            }
            PublishNode::Chrome { kind, name, state } => {
                let code = chrome_code(kind);
                if pending.chrome.len() >= pending.counts.chrome as usize
                    || !pending.seen_chrome.insert(code)
                {
                    return Err(order_refusal());
                }
                add_text(
                    pending,
                    name.len()
                        .saturating_add(state.as_ref().map_or(0, String::len)),
                )?;
                pending.chrome.push((kind, name, state));
            }
            PublishNode::Scene { kind, identity } => {
                if pending.scene.len() >= pending.counts.scene as usize
                    || !pending.seen_scene.insert(identity)
                {
                    return Err(order_refusal());
                }
                pending.scene.push((kind, identity));
            }
            PublishNode::Interactive {
                purpose,
                name,
                enabled,
            } => {
                let key = (super::protocol::purpose_code(purpose), name.clone());
                if pending.interactive.len() >= pending.counts.interactive as usize
                    || !pending.seen_interactive.insert(key)
                {
                    return Err(order_refusal());
                }
                add_text(pending, name.len())?;
                pending.interactive.push((purpose, name, enabled));
            }
            PublishNode::Notice { kind, text } => {
                if pending.notices.len() >= pending.counts.notices as usize {
                    return Err(order_refusal());
                }
                if text.chars().count() > crate::MAX_ANNOUNCEMENT_TEXT_LEN {
                    return Err(Refusal {
                        kind: ErrorKind::InvalidRequest,
                        message: "announcement too long",
                    });
                }
                add_text(pending, text.len())?;
                pending.notices.push((kind, text));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn commit_publish(&mut self, request: u64, generation: u64) -> Result<Message, Refusal> {
        let Some(pending) = self.pending.take() else {
            return Err(Refusal {
                kind: ErrorKind::InvalidRequest,
                message: ERR_NO_PUBLISH,
            });
        };
        if pending.request != request || pending.generation != generation {
            return Err(Refusal {
                kind: ErrorKind::InvalidRequest,
                message: ERR_PUBLISH_REQUEST,
            });
        }
        if PublishCounts::of(&pending) != pending.counts {
            return Err(Refusal {
                kind: ErrorKind::InvalidRequest,
                message: ERR_PUBLISH_COUNT,
            });
        }
        if let Some(live) = &self.live
            && generation <= live.generation
        {
            return Err(Refusal {
                kind: ErrorKind::StaleGeneration,
                message: ERR_STALE_GENERATION,
            });
        }

        let mut builder = SnapshotBuilder::new(Generation::from_u64(generation));
        builder
            .set_root_name(&pending.root_name)
            .map_err(map_a11y)?;
        builder
            .set_terminal(
                &pending.title,
                pending.terminal_rows.clone(),
                CursorPosition {
                    row: pending.cursor_row,
                    col: pending.cursor_col,
                },
                pending.cursor_visible,
            )
            .map_err(map_a11y)?;
        for (kind, name, state) in &pending.chrome {
            builder
                .add_chrome(*kind, name, state.as_deref())
                .map_err(map_a11y)?;
        }
        for (kind, identity) in &pending.scene {
            builder.add_scene(*kind, *identity);
        }
        for (purpose, name, enabled) in &pending.interactive {
            builder
                .add_interactive(*purpose, name, *enabled)
                .map_err(map_a11y)?;
        }
        builder.set_modal_overlay(pending.modal_overlay);
        let (snapshot, map) = builder.finish().map_err(map_a11y)?;

        let focus = bind_focus(&snapshot, &map, pending.focus.as_deref())?;
        let focus_handle = focus.as_ref().map(FocusOwner::handle);
        let notices: Vec<(AnnouncementKind, &str)> = pending
            .notices
            .iter()
            .map(|(kind, text)| (*kind, text.as_str()))
            .collect();

        let (rows, focus_id) = self.materialize(
            &snapshot,
            &map,
            focus_handle,
            pending.selection,
            pending.modal_overlay,
        )?;
        let nodes = u32::try_from(rows.len()).map_err(|_| Refusal {
            kind: ErrorKind::Budget,
            message: ERR_PUBLISH_COUNT,
        })?;
        if rows.len() > MAX_SNAPSHOT_NODES {
            return Err(Refusal {
                kind: ErrorKind::Budget,
                message: ERR_PUBLISH_COUNT,
            });
        }
        self.adapter
            .ingest(snapshot, map, focus, &notices, None)
            .map_err(map_a11y)?;
        let (removed_ids, _upserts, added, updated) = self.delta(&rows);
        let removed = u32::try_from(removed_ids.len()).unwrap_or(u32::MAX);
        self.live = Some(LiveTree {
            generation,
            rows,
            focus_id,
            selection: pending.selection.map(normalize_selection),
        });
        Ok(Message::Published {
            request,
            generation,
            nodes,
            added,
            updated,
            removed,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn materialize(
        &mut self,
        snapshot: &Snapshot,
        map: &HandleMap,
        focus: Option<ElementHandle>,
        selection: Option<Selection>,
        modal: bool,
    ) -> Result<(Vec<TreeRow>, Option<u64>), Refusal> {
        let mut collected = Vec::with_capacity(snapshot.len());
        let mut live_anchors = HashSet::with_capacity(snapshot.len());
        for handle in snapshot.preorder() {
            let anchor = map.anchor(handle).ok_or(Refusal {
                kind: ErrorKind::Internal,
                message: ERR_INTERNAL,
            })?;
            let id = self.ids.assign(anchor)?;
            live_anchors.insert(AnchorKey::of(anchor));
            collected.push((handle, anchor.clone(), id));
        }
        let root_id = collected
            .iter()
            .find(|(_, anchor, _)| matches!(anchor, crate::Anchor::Root))
            .map_or(0, |(_, _, id)| *id);
        let terminal_id = collected
            .iter()
            .find(|(_, anchor, _)| matches!(anchor, crate::Anchor::TerminalLeaf))
            .map_or(0, |(_, _, id)| *id);
        let focus_id = focus.and_then(|focus| {
            collected
                .iter()
                .find(|(handle, _, _)| *handle == focus)
                .map(|(_, _, id)| *id)
        });
        if focus.is_some() && focus_id.is_none() {
            return Err(Refusal {
                kind: ErrorKind::Internal,
                message: ERR_INTERNAL,
            });
        }
        let normalized = selection.map(normalize_selection);

        let mut rows = Vec::with_capacity(collected.len());
        for (handle, anchor, id) in &collected {
            let view = snapshot.get(*handle).ok_or(Refusal {
                kind: ErrorKind::Internal,
                message: ERR_INTERNAL,
            })?;
            let (parent, depth) = match anchor {
                crate::Anchor::Root => (0, 0),
                crate::Anchor::TerminalRow { .. } => (terminal_id, 2),
                crate::Anchor::TerminalLeaf
                | crate::Anchor::Chrome { .. }
                | crate::Anchor::Scene { .. }
                | crate::Anchor::Interactive { .. } => (root_id, 1),
            };
            if depth > MAX_TREE_DEPTH {
                return Err(Refusal {
                    kind: ErrorKind::Internal,
                    message: ERR_INTERNAL,
                });
            }
            let (role, surface) = view_role_surface(view);
            let mut states = 0_u16;
            if matches!(
                view,
                NodeView::Terminal {
                    cursor_visible: true,
                    ..
                }
            ) {
                states |= STATE_CURSOR_VISIBLE;
            }
            if is_focusable(anchor) {
                states |= STATE_FOCUSABLE;
            }
            if is_enabled(view) {
                states |= STATE_ENABLED;
            }
            if focus_id == Some(*id) {
                states |= STATE_FOCUSED;
            }
            if modal
                && let NodeView::Chrome {
                    kind: ChromeKind::Overlay,
                    ..
                } = view
            {
                states |= STATE_MODAL;
            }
            let mut row_selection = None;
            let content = match view {
                NodeView::TerminalRow { text } => {
                    if let Some(selection) = normalized
                        && let Some(span) = selection_span(selection, anchor, text)
                    {
                        states |= STATE_SELECTED;
                        row_selection = Some(span);
                    }
                    Some(text.to_owned())
                }
                _ => None,
            };
            rows.push(TreeRow {
                id: *id,
                parent,
                depth,
                role,
                surface,
                name: view_name(view).map(str::to_owned),
                content,
                state_text: view_state_text(view).map(str::to_owned),
                states,
                focus: view_focus(view),
                selection: row_selection,
                bounds: view_bounds(anchor, view),
            });
        }

        if normalized.is_some()
            && let Some(terminal) = rows
                .iter_mut()
                .find(|row| row.role == ROLE_TEXT && row.parent == root_id && row.depth == 1)
        {
            terminal.states |= STATE_SELECTED;
        }
        self.ids.retain(&live_anchors);
        Ok((rows, focus_id))
    }

    fn delta(&self, rows: &[TreeRow]) -> (Vec<u64>, Vec<TreeRow>, u32, u32) {
        let Some(baseline) = &self.baseline else {
            let added = u32::try_from(rows.len()).unwrap_or(u32::MAX);
            return (Vec::new(), rows.to_vec(), added, 0);
        };
        let live_ids: HashSet<u64> = rows.iter().map(|row| row.id).collect();
        let removed: Vec<u64> = baseline
            .rows
            .keys()
            .filter(|id| !live_ids.contains(id))
            .copied()
            .collect();
        let mut upserts = Vec::new();
        let mut added = 0_u32;
        let mut updated = 0_u32;
        for row in rows {
            match baseline.rows.get(&row.id) {
                Some(previous) if previous == row => {}
                Some(_) => {
                    updated = updated.saturating_add(1);
                    upserts.push(row.clone());
                }
                None => {
                    added = added.saturating_add(1);
                    upserts.push(row.clone());
                }
            }
        }
        (removed, upserts, added, updated)
    }

    fn ack(&mut self, generation: u64) -> Result<(), Refusal> {
        let Some(live) = &self.live else {
            return Err(Refusal {
                kind: ErrorKind::Unavailable,
                message: ERR_UNAVAILABLE,
            });
        };
        if generation != live.generation {
            return Err(Refusal {
                kind: ErrorKind::StaleGeneration,
                message: ERR_STALE_GENERATION,
            });
        }
        if self
            .baseline
            .as_ref()
            .is_some_and(|baseline| baseline.generation == generation)
        {
            return Ok(());
        }
        let rows = live.rows.iter().map(|row| (row.id, row.clone())).collect();
        self.baseline = Some(Baseline { generation, rows });
        Ok(())
    }

    fn snapshot_messages(&self, request: u64, since: u64) -> Result<Vec<Message>, Refusal> {
        let Some(live) = &self.live else {
            return Err(Refusal {
                kind: ErrorKind::Unavailable,
                message: ERR_UNAVAILABLE,
            });
        };
        if since > live.generation {
            return Err(Refusal {
                kind: ErrorKind::StaleGeneration,
                message: ERR_STALE_GENERATION,
            });
        }
        if since != 0 && since == live.generation {
            return Ok(vec![Message::DiffBegin {
                request,
                base: since,
                generation: live.generation,
                removed: 0,
                upserts: 0,
                focus: live.focus_id,
                selection: live.selection,
                pages: 0,
            }]);
        }
        if since != 0
            && self
                .baseline
                .as_ref()
                .is_some_and(|baseline| baseline.generation == since)
        {
            return Ok(self.diff_messages(request, live));
        }
        Ok(Self::full_snapshot_messages(request, live))
    }

    fn full_snapshot_messages(request: u64, live: &LiveTree) -> Vec<Message> {
        let nodes = u32::try_from(live.rows.len()).unwrap_or(u32::MAX);
        let pages = page_count(live.rows.len(), MAX_SNAPSHOT_PAGE_ROWS);
        let mut replies = Vec::with_capacity(pages as usize + 1);
        replies.push(Message::SnapshotBegin {
            request,
            generation: live.generation,
            nodes,
            focus: live.focus_id,
            selection: live.selection,
            pages,
        });
        for (index, chunk) in live.rows.chunks(MAX_SNAPSHOT_PAGE_ROWS).enumerate() {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            replies.push(Message::SnapshotPage {
                request,
                index,
                rows: chunk.to_vec(),
                last: index + 1 == pages,
            });
        }
        replies
    }

    fn diff_messages(&self, request: u64, live: &LiveTree) -> Vec<Message> {
        let Some(baseline) = &self.baseline else {
            return Self::full_snapshot_messages(request, live);
        };
        let (removed_ids, upserts, _, _) = self.delta(&live.rows);
        let removed = u32::try_from(removed_ids.len()).unwrap_or(u32::MAX);
        let pages = u32::try_from(removed_ids.len().div_ceil(MAX_DIFF_PAGE_REMOVED))
            .unwrap_or(u32::MAX)
            .max(u32::try_from(upserts.len().div_ceil(MAX_DIFF_PAGE_ROWS)).unwrap_or(u32::MAX));
        let mut replies = Vec::with_capacity(pages as usize + 1);
        replies.push(Message::DiffBegin {
            request,
            base: baseline.generation,
            generation: live.generation,
            removed,
            upserts: u32::try_from(upserts.len()).unwrap_or(u32::MAX),
            focus: live.focus_id,
            selection: live.selection,
            pages,
        });
        if pages == 0 {
            return replies;
        }
        for index in 0..pages as usize {
            let removed_chunk = removed_ids
                .chunks(MAX_DIFF_PAGE_REMOVED)
                .nth(index)
                .unwrap_or_default()
                .to_vec();
            let upsert_chunk = upserts
                .chunks(MAX_DIFF_PAGE_ROWS)
                .nth(index)
                .unwrap_or_default()
                .to_vec();
            replies.push(Message::DiffPage {
                request,
                index: u32::try_from(index).unwrap_or(u32::MAX),
                removed: removed_chunk,
                upserts: upsert_chunk,
                last: u32::try_from(index).unwrap_or(u32::MAX) + 1 == pages,
            });
        }
        replies
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

fn order_refusal() -> Refusal {
    Refusal {
        kind: ErrorKind::InvalidRequest,
        message: ERR_PUBLISH_ORDER,
    }
}

fn denied(request: u64) -> Message {
    Message::Error {
        request,
        kind: ErrorKind::Denied,
        message: ERR_DENIED.to_owned(),
    }
}

fn refusal_message(request: u64, refusal: Refusal) -> Message {
    Message::Error {
        request,
        kind: refusal.kind,
        message: refusal.message.to_owned(),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_a11y(error: A11yError) -> Refusal {
    match error {
        A11yError::UnmappedSceneKind => Refusal {
            kind: ErrorKind::InvalidRequest,
            message: "unmapped scene kind",
        },
        A11yError::NameTooLong { .. } => Refusal {
            kind: ErrorKind::InvalidRequest,
            message: "accessible name too long",
        },
        A11yError::TooManyNodes { .. } => Refusal {
            kind: ErrorKind::Budget,
            message: "snapshot node cap exceeded",
        },
        A11yError::UnknownHandle => Refusal {
            kind: ErrorKind::InvalidRequest,
            message: "focus label unmatched",
        },
        A11yError::FocusMismatch => Refusal {
            kind: ErrorKind::InvalidRequest,
            message: "focus target cannot own focus",
        },
        A11yError::StaleGeneration { .. } => Refusal {
            kind: ErrorKind::StaleGeneration,
            message: ERR_STALE_GENERATION,
        },
        A11yError::AnnouncementTooLong { .. } => Refusal {
            kind: ErrorKind::InvalidRequest,
            message: "announcement too long",
        },
        A11yError::InvalidStructure => Refusal {
            kind: ErrorKind::InvalidRequest,
            message: "invalid snapshot structure",
        },
        A11yError::PermissionDenied | A11yError::CapabilityDenied => Refusal {
            kind: ErrorKind::Denied,
            message: ERR_DENIED,
        },
        A11yError::UnknownAction { .. } | A11yError::NotEnabled | A11yError::NotActivatable => {
            Refusal {
                kind: ErrorKind::Unsupported,
                message: "operation unsupported",
            }
        }
    }
}

fn bind_focus(
    snapshot: &Snapshot,
    map: &HandleMap,
    label: Option<&str>,
) -> Result<Option<FocusOwner>, Refusal> {
    let Some(label) = label else {
        return Ok(None);
    };
    let mut matched = false;
    for handle in snapshot.preorder() {
        let Some(view) = snapshot.get(handle) else {
            continue;
        };
        if view_name(view) != Some(label) {
            continue;
        }
        matched = true;
        match FocusOwner::bind(snapshot, map, handle) {
            Ok(owner) => return Ok(Some(owner)),
            Err(A11yError::FocusMismatch) => {}
            Err(error) => return Err(map_a11y(error)),
        }
    }
    if matched {
        Err(Refusal {
            kind: ErrorKind::InvalidRequest,
            message: "focus target cannot own focus",
        })
    } else {
        Err(Refusal {
            kind: ErrorKind::InvalidRequest,
            message: "focus label unmatched",
        })
    }
}

fn view_role_surface(view: NodeView<'_>) -> (u8, u8) {
    match view {
        NodeView::Root { .. } => (ROLE_GROUP, SURFACE_NONE),
        NodeView::Terminal { .. } | NodeView::TerminalRow { .. } => (ROLE_TEXT, SURFACE_NONE),
        NodeView::Chrome { kind, .. } => (ROLE_NONE, chrome_code(kind)),
        NodeView::Scene { role, .. } => (role_code(role), SURFACE_NONE),
        NodeView::Interactive { purpose, .. } => (role_code(purpose.role()), SURFACE_NONE),
    }
}

fn view_name(view: NodeView<'_>) -> Option<&str> {
    match view {
        NodeView::Root { name }
        | NodeView::Chrome { name, .. }
        | NodeView::Interactive { name, .. } => Some(name),
        NodeView::Terminal { title, .. } => Some(title),
        NodeView::TerminalRow { .. } | NodeView::Scene { .. } => None,
    }
}

fn view_state_text(view: NodeView<'_>) -> Option<&str> {
    match view {
        NodeView::Chrome { state, .. } => state,
        _ => None,
    }
}

fn view_focus(view: NodeView<'_>) -> Option<FocusCell> {
    match view {
        NodeView::Terminal {
            cursor,
            cursor_visible,
            ..
        } if cursor_visible => Some(FocusCell {
            row: cursor.row,
            col: cursor.col,
        }),
        _ => None,
    }
}

fn view_bounds(anchor: &crate::Anchor, view: NodeView<'_>) -> Option<Bounds> {
    match (anchor, view) {
        (crate::Anchor::TerminalRow { row }, NodeView::TerminalRow { text }) => {
            let width = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
            Some(Bounds {
                x: 0,
                y: *row,
                width,
                height: 1,
                unit: BOUNDS_UNIT_CHARS,
            })
        }
        _ => None,
    }
}

fn is_focusable(anchor: &crate::Anchor) -> bool {
    matches!(
        anchor,
        crate::Anchor::TerminalLeaf
            | crate::Anchor::TerminalRow { .. }
            | crate::Anchor::Interactive { .. }
    )
}

fn is_enabled(view: NodeView<'_>) -> bool {
    match view {
        NodeView::Interactive { enabled, .. } => enabled,
        NodeView::Terminal { .. } | NodeView::TerminalRow { .. } => true,
        NodeView::Root { .. } | NodeView::Chrome { .. } | NodeView::Scene { .. } => false,
    }
}

fn normalize_selection(selection: Selection) -> Selection {
    let (anchor_row, anchor_col, focus_row, focus_col) = selection.normalized();
    Selection {
        anchor_row,
        anchor_col,
        focus_row,
        focus_col,
    }
}

fn selection_span(
    selection: Selection,
    anchor: &crate::Anchor,
    text: &str,
) -> Option<SelectionSpan> {
    let crate::Anchor::TerminalRow { row } = anchor else {
        return None;
    };
    let row = *row;
    let (start_row, start_col, end_row, end_col) = selection.normalized();
    if row < start_row || row > end_row {
        return None;
    }
    let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    let start = if row == start_row { start_col } else { 0 };
    let end = if row == end_row { end_col } else { chars };
    let end = end.min(chars);
    if start >= end {
        return None;
    }
    Some(SelectionSpan {
        start_col: start,
        end_col: end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::protocol::{HELLO_CAPABILITY_GRANTED, PublishNode, Selection};
    use crate::{ChromeKind, SceneKind};

    fn now() -> Instant {
        Instant::now()
    }

    fn hello(mode: u8) -> SessionOutcome {
        Session::new().handle(
            Message::Hello {
                min: 1,
                max: 1,
                component: "a11y".to_owned(),
                version: "test".to_owned(),
                mode,
            },
            now(),
        )
    }

    #[test]
    fn handshake_reports_capabilities() {
        let outcome = hello(HELLO_CAPABILITY_GRANTED);
        assert_eq!(outcome.close, None);
        assert!(matches!(
            outcome.replies.as_slice(),
            [Message::HelloAck { capabilities, .. }] if *capabilities != 0
        ));
    }

    #[test]
    fn safe_mode_is_deprived() {
        let mut session = Session::new();
        let outcome = session.handle(
            Message::Hello {
                min: 1,
                max: 1,
                component: "a11y".to_owned(),
                version: "test".to_owned(),
                mode: HELLO_SAFE_MODE | HELLO_CAPABILITY_GRANTED,
            },
            now(),
        );
        assert!(matches!(
            outcome.replies.as_slice(),
            [Message::HelloAck {
                capabilities: 0,
                ..
            }]
        ));
        let outcome = session.handle(
            Message::PublishBegin {
                request: 1,
                generation: 0,
                root_name: "window".to_owned(),
                title: "shell".to_owned(),
                cursor_row: 0,
                cursor_col: 0,
                cursor_visible: true,
                terminal_rows: 0,
                chrome: 0,
                scene: 0,
                interactive: 0,
                notices: 0,
                modal_overlay: false,
                focus: None,
                selection: None,
            },
            now(),
        );
        assert!(matches!(
            outcome.replies.as_slice(),
            [Message::Error {
                kind: ErrorKind::Denied,
                ..
            }]
        ));
    }

    #[test]
    fn full_publish_snapshot_and_diff() {
        let mut session = Session::new();
        hello_session(&mut session);
        publish(&mut session, 1, 1, &[("$ echo hi", 0)]);
        let outcome = session.handle(
            Message::SnapshotRequest {
                request: 2,
                since: 0,
            },
            now(),
        );
        assert!(matches!(
            outcome.replies.first(),
            Some(Message::SnapshotBegin { nodes, .. }) if *nodes >= 2
        ));
    }

    fn hello_session(session: &mut Session) {
        let outcome = session.handle(
            Message::Hello {
                min: 1,
                max: 1,
                component: "a11y".to_owned(),
                version: "test".to_owned(),
                mode: HELLO_CAPABILITY_GRANTED,
            },
            now(),
        );
        assert_eq!(outcome.close, None);
    }

    fn publish(session: &mut Session, request: u64, generation: u64, rows: &[(&str, u32)]) {
        let outcome = session.handle(
            Message::PublishBegin {
                request,
                generation,
                root_name: "window".to_owned(),
                title: "shell".to_owned(),
                cursor_row: 0,
                cursor_col: 0,
                cursor_visible: true,
                terminal_rows: u32::try_from(rows.len()).unwrap_or(0),
                chrome: 1,
                scene: 0,
                interactive: 0,
                notices: 0,
                modal_overlay: false,
                focus: None,
                selection: None,
            },
            now(),
        );
        assert!(outcome.replies.is_empty(), "{:?}", outcome.replies);
        for (text, row) in rows {
            let outcome = session.handle(
                Message::PublishNode {
                    request,
                    node: PublishNode::TerminalRow {
                        row: *row,
                        text: (*text).to_owned(),
                    },
                },
                now(),
            );
            assert!(outcome.replies.is_empty(), "{:?}", outcome.replies);
        }
        let _ = session.handle(
            Message::PublishNode {
                request,
                node: PublishNode::Chrome {
                    kind: ChromeKind::Bar,
                    name: "status".to_owned(),
                    state: None,
                },
            },
            now(),
        );
        let outcome = session.handle(
            Message::PublishCommit {
                request,
                generation,
            },
            now(),
        );
        assert!(
            matches!(outcome.replies.as_slice(), [Message::Published { .. }]),
            "{:?}",
            outcome.replies
        );
    }

    #[test]
    fn stale_generation_is_refused() {
        let mut session = Session::new();
        hello_session(&mut session);
        publish(&mut session, 1, 2, &[]);
        let outcome = session.handle(
            Message::PublishBegin {
                request: 2,
                generation: 1,
                root_name: "window".to_owned(),
                title: "shell".to_owned(),
                cursor_row: 0,
                cursor_col: 0,
                cursor_visible: true,
                terminal_rows: 0,
                chrome: 0,
                scene: 0,
                interactive: 0,
                notices: 0,
                modal_overlay: false,
                focus: None,
                selection: None,
            },
            now(),
        );
        assert!(matches!(
            outcome.replies.as_slice(),
            [Message::Error {
                kind: ErrorKind::StaleGeneration,
                ..
            }]
        ));
    }

    #[test]
    fn selection_is_clipped_per_row() {
        let selection = Selection {
            anchor_row: 0,
            anchor_col: 2,
            focus_row: 1,
            focus_col: 3,
        };
        let first = selection_span(selection, &crate::Anchor::TerminalRow { row: 0 }, "abcdef")
            .expect("first row");
        assert_eq!(
            first,
            SelectionSpan {
                start_col: 2,
                end_col: 6
            }
        );
        let second = selection_span(selection, &crate::Anchor::TerminalRow { row: 1 }, "abcdef")
            .expect("second row");
        assert_eq!(
            second,
            SelectionSpan {
                start_col: 0,
                end_col: 3
            }
        );
        assert!(
            selection_span(selection, &crate::Anchor::TerminalRow { row: 2 }, "abcdef").is_none()
        );
    }

    #[test]
    fn scene_unknown_fails_closed() {
        let mut session = Session::new();
        hello_session(&mut session);
        let outcome = session.handle(
            Message::PublishBegin {
                request: 1,
                generation: 0,
                root_name: "window".to_owned(),
                title: "shell".to_owned(),
                cursor_row: 0,
                cursor_col: 0,
                cursor_visible: true,
                terminal_rows: 0,
                chrome: 0,
                scene: 1,
                interactive: 0,
                notices: 0,
                modal_overlay: false,
                focus: None,
                selection: None,
            },
            now(),
        );
        assert!(outcome.replies.is_empty());
        let _ = session.handle(
            Message::PublishNode {
                request: 1,
                node: PublishNode::Scene {
                    kind: SceneKind::Unknown,
                    identity: 1,
                },
            },
            now(),
        );
        let outcome = session.handle(
            Message::PublishCommit {
                request: 1,
                generation: 0,
            },
            now(),
        );
        assert!(matches!(
            outcome.replies.as_slice(),
            [Message::Error {
                kind: ErrorKind::InvalidRequest,
                ..
            }]
        ));
    }
}
