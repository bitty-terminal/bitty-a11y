//! Accessibility component wire protocol v1 (Core host <-> `bitty-a11y`).
//!
//! The component is a stdio coprocess: the host writes requests to its stdin
//! and reads framed messages from its stdout. stderr stays diagnostic and is
//! never protocol. This module is the authoritative schema and codec; it is
//! hand-written (`std` only), forbids panics on hostile input, and fails
//! closed on every bound.
//!
//! # Framing
//!
//! ```text
//! frame   = length:u32be payload[length]
//! payload = tag:u8 fields...
//! ```
//!
//! `length` is 1..=[`MAX_FRAME_BYTES`]. A larger or zero length is rejected
//! from the four-byte header alone; no payload byte is read or allocated.
//!
//! # Field encoding
//!
//! - Integers are big-endian fixed width (`u8`, `u16`, `u32`, `u64`).
//! - `bool` is one byte, `0x00` or `0x01`; any other value is rejected.
//! - `str` is `len:u32be` + UTF-8 bytes, every length bounded per field.
//! - `bytes` is `len:u32be` + raw bytes, bounded per field.
//! - Lists are `count:u32be` + elements, count bounded per field.
//! - An optional value is `0x00` (absent) or `0x01` followed by the value.
//!
//! # Messages
//!
//! | Tag    | Message         | Fields (in order)                                                                                                                       | Direction  |
//! |--------|-----------------|-----------------------------------------------------------------------------------------------------------------------------------------|------------|
//! | `0x01` | `Hello`         | `min:u16 max:u16 component:str version:str mode:u8`                                                                                      | host->comp |
//! | `0x02` | `HelloAck`      | `protocol:u16 component:str version:str capabilities:u8`                                                                                 | comp->host |
//! | `0x10` | `PublishBegin`  | `request:u64 generation:u64 root:str title:str cursor_row:u32 cursor_col:u32 cursor_visible:bool rows:u32 chrome:u32 scene:u32 interactive:u32 notices:u32 modal:bool focus:opt<str> selection:opt<Selection>` | host->comp |
//! | `0x11` | `PublishNode`   | `request:u64 node:u8 fields...`                                                                                                          | host->comp |
//! | `0x12` | `PublishCommit` | `request:u64 generation:u64`                                                                                                             | host->comp |
//! | `0x13` | `SnapshotRequest` | `request:u64 since:u64`                                                                                                                | host->comp |
//! | `0x14` | `DiffAck`       | `generation:u64`                                                                                                                         | host->comp |
//! | `0x3F` | `Shutdown`      | (none)                                                                                                                                  | host->comp |
//! | `0x20` | `Published`     | `request:u64 generation:u64 nodes:u32 added:u32 updated:u32 removed:u32`                                                                 | comp->host |
//! | `0x21` | `SnapshotBegin` | `request:u64 generation:u64 nodes:u32 focus:opt<u64> selection:opt<Selection> pages:u32`                                                 | comp->host |
//! | `0x22` | `SnapshotPage`  | `request:u64 index:u32 rows:list<TreeRow> last:bool`                                                                                     | comp->host |
//! | `0x23` | `DiffBegin`     | `request:u64 base:u64 generation:u64 removed:u32 upserts:u32 focus:opt<u64> selection:opt<Selection> pages:u32`                          | comp->host |
//! | `0x24` | `DiffPage`      | `request:u64 index:u32 removed:list<u64> upserts:list<TreeRow> last:bool`                                                                | comp->host |
//! | `0x2F` | `Error`         | `request:u64 kind:u8 message:str`                                                                                                        | comp->host |
//! | `0x3E` | `ShutdownAck`   | (none)                                                                                                                                  | comp->host |
//!
//! `request` ids are non-zero and host-assigned. [`CONNECTION_ID`] (`0`)
//! marks a connection-level [`Message::Error`] (handshake, framing, or
//! protocol failure). `generation` is the projection generation Core reports;
//! a publication must advance it, and a host that presents an older or unknown
//! generation gets a typed error or a full snapshot, never a fabricated
//! delta.
//!
//! [`Message::PublishNode`] carries exactly one projection entry:
//!
//! | Node code | Entry                                                                                       |
//! |-----------|---------------------------------------------------------------------------------------------|
//! | `0x01`    | `TerminalRow { row:u32 text:str }`                                                          |
//! | `0x02`    | `Chrome { kind:u8 name:str state:opt<str> }`                                                |
//! | `0x03`    | `Scene { kind:u8 identity:u64 }`                                                            |
//! | `0x04`    | `Interactive { purpose:u8 name:str enabled:bool }`                                          |
//! | `0x05`    | `Notice { kind:u8 text:str }`                                                               |
//!
//! `PublishBegin` declares exact entry counts; the component rejects a
//! publish whose node stream disagrees with the declaration, so no partial
//! projection is ever presented.
//!
//! # Authority and capability boundary
//!
//! This protocol carries **no file operations, no descriptor passing, no
//! environment, no argv, no URL, no socket, and no ambient command
//! language**. It names projection content, generations, request ids, and
//! bounded tree rows only. The host owns installation, digest verification,
//! capability grants, consent, safe-mode denial, and the platform
//! accessibility bridge; the component validates what it is handed and never
//! widens it. Error messages are bounded static diagnostics, never echoed
//! names, text, or paths.
//!
//! `Hello.mode` bit `0x01` declares safe mode and bit `0x02` declares that the
//! host granted the accessibility exposure capability. Safe mode, a missing
//! grant, or both make the session **deprived**: the handshake completes with
//! zero capabilities, projection exchanges fail closed with `denied`, and
//! shutdown still works.
//!
//! # Bounds
//!
//! - [`MAX_FRAME_BYTES`] (256 KiB) per frame, checked before allocation.
//! - [`MAX_TREE_DEPTH`] (4) levels; the projection is two levels below the
//!   root by construction.
//! - [`MAX_SNAPSHOT_NODES`](crate::MAX_SNAPSHOT_NODES) (4096) nodes.
//! - [`MAX_SNAPSHOT_PAGE_ROWS`] (16) rows per snapshot page,
//!   [`MAX_DIFF_PAGE_ROWS`] (16) upserts and [`MAX_DIFF_PAGE_REMOVED`] (64)
//!   removed ids per diff page.
//! - [`MAX_ROW_TEXT_BYTES`] (8 KiB) per terminal run, bounded names, and a
//!   [`MAX_PUBLISH_TEXT_BYTES`] (1 MiB) total publication budget.
//! - [`MAX_PUBLISH_ENTRIES`] (4104) staged publication entries.
//! - A request rate gate in [`server`](crate::component::server).
//!
//! All bounds are checked on both encode and decode, so a peer can never emit
//! a frame the other side would refuse.

use std::fmt;
use std::io::{self, Read, Write};

use crate::MAX_ACCESSIBLE_NAME_LEN;
use crate::MAX_PENDING_ANNOUNCEMENTS;
use crate::MAX_SNAPSHOT_NODES;
use crate::announce::AnnouncementKind;
use crate::snapshot::{ChromeKind, InteractivePurpose, SceneKind};

/// Wire protocol version implemented by this crate.
pub const PROTOCOL_VERSION: u16 = 1;

/// Size of the big-endian `u32` length prefix in front of every payload.
pub const FRAME_HEADER_BYTES: usize = 4;

/// Maximum payload length of one frame (256 KiB, matching the other
/// component codecs).
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Maximum component name bytes (`[a-z][a-z0-9-]{0,31}`).
pub const MAX_COMPONENT_NAME_BYTES: usize = 32;

/// Maximum version string bytes in `Hello` / `HelloAck`.
pub const MAX_VERSION_BYTES: usize = 64;

/// Maximum `Error.message` bytes.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 256;

/// Maximum bytes of one accessible name (256 four-byte scalars).
pub const MAX_NAME_BYTES: usize = MAX_ACCESSIBLE_NAME_LEN * 4;

/// Maximum bytes of one terminal run.
pub const MAX_ROW_TEXT_BYTES: usize = 8 * 1024;

/// Maximum bytes of an announced chrome state.
pub const MAX_STATE_TEXT_BYTES: usize = MAX_NAME_BYTES;

/// Maximum tree depth a row may declare.
pub const MAX_TREE_DEPTH: u8 = 4;

/// Rows carried by one snapshot page.
pub const MAX_SNAPSHOT_PAGE_ROWS: usize = 16;

/// Upserted rows carried by one diff page.
pub const MAX_DIFF_PAGE_ROWS: usize = 16;

/// Removed ids carried by one diff page.
pub const MAX_DIFF_PAGE_REMOVED: usize = 64;

/// Maximum staged entries in one publication exchange.
pub const MAX_PUBLISH_ENTRIES: usize = MAX_SNAPSHOT_NODES + MAX_PENDING_ANNOUNCEMENTS;

/// Maximum combined name, state, and run text bytes in one publication.
pub const MAX_PUBLISH_TEXT_BYTES: usize = 1024 * 1024;

/// Highest grid row a cursor or selection coordinate may name (exclusive).
pub const MAX_GRID_ROWS: u32 = 1_000_000;

/// Highest grid column a cursor or selection coordinate may name (exclusive).
pub const MAX_GRID_COLS: u32 = 65_536;

/// Maximum pages one snapshot or diff response may contain (4096 / 16).
pub const MAX_PAGES: u32 = 256;

// Keep the page bound tied to the model bound without a lossy const cast.
const _: () = assert!(MAX_SNAPSHOT_NODES == 4096);
const _: () = assert!(MAX_SNAPSHOT_PAGE_ROWS == 16);

/// `Hello.mode` bit: the host is in safe mode.
pub const HELLO_SAFE_MODE: u8 = 0x01;

/// `Hello.mode` bit: the host granted the accessibility exposure capability.
pub const HELLO_CAPABILITY_GRANTED: u8 = 0x02;

/// Defined `Hello.mode` bits; every other bit is refused.
pub const HELLO_MODE_MASK: u8 = HELLO_SAFE_MODE | HELLO_CAPABILITY_GRANTED;

/// `HelloAck.capabilities` bit: full bounded snapshots.
pub const CAPABILITY_SNAPSHOT: u8 = 0x01;

/// `HelloAck.capabilities` bit: bounded diffs and acknowledgements.
pub const CAPABILITY_DIFF: u8 = 0x02;

/// `HelloAck.capabilities` bit: selection state.
pub const CAPABILITY_SELECTION: u8 = 0x04;

/// `HelloAck.capabilities` bit: coalesced announcements (chrome state text).
pub const CAPABILITY_ANNOUNCEMENTS: u8 = 0x08;

/// Every capability bit defined in v1.
pub const CAPABILITY_MASK: u8 =
    CAPABILITY_SNAPSHOT | CAPABILITY_DIFF | CAPABILITY_SELECTION | CAPABILITY_ANNOUNCEMENTS;

// ── tags ────────────────────────────────────────────────────────────────────

/// `Hello` tag.
pub const TAG_HELLO: u8 = 0x01;
/// `HelloAck` tag.
pub const TAG_HELLO_ACK: u8 = 0x02;
/// `PublishBegin` tag.
pub const TAG_PUBLISH_BEGIN: u8 = 0x10;
/// `PublishNode` tag.
pub const TAG_PUBLISH_NODE: u8 = 0x11;
/// `PublishCommit` tag.
pub const TAG_PUBLISH_COMMIT: u8 = 0x12;
/// `SnapshotRequest` tag.
pub const TAG_SNAPSHOT_REQUEST: u8 = 0x13;
/// `DiffAck` tag.
pub const TAG_DIFF_ACK: u8 = 0x14;
/// `Published` tag.
pub const TAG_PUBLISHED: u8 = 0x20;
/// `SnapshotBegin` tag.
pub const TAG_SNAPSHOT_BEGIN: u8 = 0x21;
/// `SnapshotPage` tag.
pub const TAG_SNAPSHOT_PAGE: u8 = 0x22;
/// `DiffBegin` tag.
pub const TAG_DIFF_BEGIN: u8 = 0x23;
/// `DiffPage` tag.
pub const TAG_DIFF_PAGE: u8 = 0x24;
/// `Error` tag.
pub const TAG_ERROR: u8 = 0x2F;
/// `ShutdownAck` tag.
pub const TAG_SHUTDOWN_ACK: u8 = 0x3E;
/// `Shutdown` tag.
pub const TAG_SHUTDOWN: u8 = 0x3F;

/// Connection-level `Error` frame request id.
pub const CONNECTION_ID: u64 = 0;

// ── tree-row vocabulary ─────────────────────────────────────────────────────

/// Row role code: running text.
pub const ROLE_TEXT: u8 = 0;
/// Row role code: grouping container.
pub const ROLE_GROUP: u8 = 1;
/// Row role code: code block.
pub const ROLE_CODE: u8 = 2;
/// Row role code: table.
pub const ROLE_TABLE: u8 = 3;
/// Row role code: list.
pub const ROLE_LIST: u8 = 4;
/// Row role code: button.
pub const ROLE_BUTTON: u8 = 5;
/// Row role code: text input.
pub const ROLE_INPUT: u8 = 6;
/// Row role code: separator.
pub const ROLE_SEPARATOR: u8 = 7;
/// Row role code: image placement.
pub const ROLE_IMAGE: u8 = 8;
/// Row role code: no accessible role (a chrome surface identified by
/// [`SURFACE_*`] instead).
pub const ROLE_NONE: u8 = 9;

/// Surface code: not a chrome surface.
pub const SURFACE_NONE: u8 = 0;
/// Surface code: status bar.
pub const SURFACE_BAR: u8 = 1;
/// Surface code: side rail.
pub const SURFACE_RAIL: u8 = 2;
/// Surface code: tab strip.
pub const SURFACE_TABLIST: u8 = 3;
/// Surface code: notification.
pub const SURFACE_NOTIFICATION: u8 = 4;
/// Surface code: modal overlay.
pub const SURFACE_OVERLAY: u8 = 5;

/// State bit: the element may receive focus.
pub const STATE_FOCUSABLE: u16 = 0x0001;
/// State bit: the element currently has focus.
pub const STATE_FOCUSED: u16 = 0x0002;
/// State bit: an interactive element is enabled.
pub const STATE_ENABLED: u16 = 0x0004;
/// State bit: the element (or its text span) is selected.
pub const STATE_SELECTED: u16 = 0x0008;
/// State bit: the element is the active modal overlay.
pub const STATE_MODAL: u16 = 0x0010;
/// State bit: the terminal cursor is visible at [`TreeRow::focus`].
pub const STATE_CURSOR_VISIBLE: u16 = 0x0020;

/// Every state bit defined in v1.
pub const STATE_MASK: u16 = STATE_FOCUSABLE
    | STATE_FOCUSED
    | STATE_ENABLED
    | STATE_SELECTED
    | STATE_MODAL
    | STATE_CURSOR_VISIBLE;

/// Bounds unit: characters (a logical text extent, never pixels).
pub const BOUNDS_UNIT_CHARS: u8 = 0;

/// A top-level selection over terminal cells.
///
/// Coordinates are zero-based grid cells; `anchor` is where the selection
/// started and `focus` where it currently ends. Rows are bound by
/// [`MAX_GRID_ROWS`] and columns by [`MAX_GRID_COLS`]. The range is
/// half-open at the focus cell for display purposes: the component emits
/// `[start_col, end_col)` spans per terminal row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Selection {
    /// Row where the selection started.
    pub anchor_row: u32,
    /// Column where the selection started.
    pub anchor_col: u32,
    /// Row where the selection currently ends.
    pub focus_row: u32,
    /// Column where the selection currently ends.
    pub focus_col: u32,
}

impl Selection {
    /// Normalizes the range so the start is row-major before the end.
    #[must_use]
    pub fn normalized(self) -> (u32, u32, u32, u32) {
        let start = (self.anchor_row, self.anchor_col);
        let end = (self.focus_row, self.focus_col);
        if start <= end {
            (
                self.anchor_row,
                self.anchor_col,
                self.focus_row,
                self.focus_col,
            )
        } else {
            (
                self.focus_row,
                self.focus_col,
                self.anchor_row,
                self.anchor_col,
            )
        }
    }
}

/// One cursor cell carried by the terminal row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FocusCell {
    /// Zero-based grid row.
    pub row: u32,
    /// Zero-based grid column.
    pub col: u32,
}

/// One selected span on a terminal row (`[start_col, end_col)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SelectionSpan {
    /// First selected column (inclusive).
    pub start_col: u32,
    /// Column after the last selected one (exclusive).
    pub end_col: u32,
}

/// Logical text bounds for a row (characters, never pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Bounds {
    /// Horizontal character offset.
    pub x: u32,
    /// Row ordinal.
    pub y: u32,
    /// Width in characters.
    pub width: u32,
    /// Height in rows.
    pub height: u32,
    /// Unit code; v1 defines [`BOUNDS_UNIT_CHARS`] only.
    pub unit: u8,
}

/// One canonical accessibility tree row.
///
/// `id` is the projection anchor's stable id: it survives a generation
/// advance while the node identity survives, and it is never reused after a
/// node disappears. `parent` is `0` for the root and otherwise the parent
/// row id; `depth` is bounded by [`MAX_TREE_DEPTH`]. `role` is the fixed v1
/// role vocabulary ([`ROLE_TEXT`]..[`ROLE_IMAGE`]) or [`ROLE_NONE`] for a
/// chrome surface, which is identified by `surface` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Stable element id (non-zero).
    pub id: u64,
    /// Parent element id (`0` for the root).
    pub parent: u64,
    /// Tree depth (root is `0`).
    pub depth: u8,
    /// Role code.
    pub role: u8,
    /// Chrome surface code ([`SURFACE_NONE`] outside chrome).
    pub surface: u8,
    /// Accessible name, when the node carries one.
    pub name: Option<String>,
    /// Terminal row content, when the node is a text row.
    pub content: Option<String>,
    /// Announced chrome state, when the node carries one.
    pub state_text: Option<String>,
    /// State bits.
    pub states: u16,
    /// Terminal cursor position, only on the terminal leaf.
    pub focus: Option<FocusCell>,
    /// Selected span, only on selected terminal rows.
    pub selection: Option<SelectionSpan>,
    /// Logical text bounds, only on terminal rows in v1.
    pub bounds: Option<Bounds>,
}

// ── publish vocabulary ──────────────────────────────────────────────────────

/// One staged publication entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishNode {
    /// One terminal text run, keyed by row ordinal.
    TerminalRow {
        /// Zero-based grid row.
        row: u32,
        /// Readable row content.
        text: String,
    },
    /// One chrome surface.
    Chrome {
        /// Surface kind.
        kind: ChromeKind,
        /// Accessible name.
        name: String,
        /// Announced state, if any.
        state: Option<String>,
    },
    /// One scene node.
    Scene {
        /// Producer scene kind.
        kind: SceneKind,
        /// Producer-assigned identity.
        identity: u64,
    },
    /// One interactive node.
    Interactive {
        /// Declared purpose.
        purpose: InteractivePurpose,
        /// Accessible name.
        name: String,
        /// Whether the control can be activated.
        enabled: bool,
    },
    /// One coalesced announcement.
    Notice {
        /// Transition kind.
        kind: AnnouncementKind,
        /// Bounded notice text.
        text: String,
    },
}

impl PublishNode {
    const TAG_TERMINAL_ROW: u8 = 0x01;
    const TAG_CHROME: u8 = 0x02;
    const TAG_SCENE: u8 = 0x03;
    const TAG_INTERACTIVE: u8 = 0x04;
    const TAG_NOTICE: u8 = 0x05;

    const fn code(&self) -> u8 {
        match self {
            Self::TerminalRow { .. } => Self::TAG_TERMINAL_ROW,
            Self::Chrome { .. } => Self::TAG_CHROME,
            Self::Scene { .. } => Self::TAG_SCENE,
            Self::Interactive { .. } => Self::TAG_INTERACTIVE,
            Self::Notice { .. } => Self::TAG_NOTICE,
        }
    }
}

// ── closed wire vocabularies ────────────────────────────────────────────────

/// Chrome surface code.
#[must_use]
pub const fn chrome_code(kind: ChromeKind) -> u8 {
    match kind {
        ChromeKind::Bar => SURFACE_BAR,
        ChromeKind::Rail => SURFACE_RAIL,
        ChromeKind::TabStrip => SURFACE_TABLIST,
        ChromeKind::Notification => SURFACE_NOTIFICATION,
        ChromeKind::Overlay => SURFACE_OVERLAY,
    }
}

/// Chrome surface for a wire code.
#[must_use]
pub const fn chrome_from_code(code: u8) -> Option<ChromeKind> {
    match code {
        SURFACE_BAR => Some(ChromeKind::Bar),
        SURFACE_RAIL => Some(ChromeKind::Rail),
        SURFACE_TABLIST => Some(ChromeKind::TabStrip),
        SURFACE_NOTIFICATION => Some(ChromeKind::Notification),
        SURFACE_OVERLAY => Some(ChromeKind::Overlay),
        _ => None,
    }
}

/// Scene kind wire code.
#[must_use]
pub const fn scene_code(kind: SceneKind) -> u8 {
    match kind {
        SceneKind::Text => 0,
        SceneKind::Row => 1,
        SceneKind::Column => 2,
        SceneKind::Block => 3,
        SceneKind::Image => 4,
        SceneKind::CodeBlock => 5,
        SceneKind::Table => 6,
        SceneKind::List => 7,
        SceneKind::Rule => 8,
        SceneKind::Unknown => 9,
    }
}

/// Scene kind for a wire code.
#[must_use]
pub const fn scene_from_code(code: u8) -> Option<SceneKind> {
    match code {
        0 => Some(SceneKind::Text),
        1 => Some(SceneKind::Row),
        2 => Some(SceneKind::Column),
        3 => Some(SceneKind::Block),
        4 => Some(SceneKind::Image),
        5 => Some(SceneKind::CodeBlock),
        6 => Some(SceneKind::Table),
        7 => Some(SceneKind::List),
        8 => Some(SceneKind::Rule),
        9 => Some(SceneKind::Unknown),
        _ => None,
    }
}

/// Interactive purpose wire code.
#[must_use]
pub const fn purpose_code(purpose: InteractivePurpose) -> u8 {
    match purpose {
        InteractivePurpose::Button => 1,
        InteractivePurpose::Input => 2,
    }
}

/// Interactive purpose for a wire code.
#[must_use]
pub const fn purpose_from_code(code: u8) -> Option<InteractivePurpose> {
    match code {
        1 => Some(InteractivePurpose::Button),
        2 => Some(InteractivePurpose::Input),
        _ => None,
    }
}

/// Announcement kind wire code.
#[must_use]
pub const fn notice_code(kind: AnnouncementKind) -> u8 {
    match kind {
        AnnouncementKind::Focus => 1,
        AnnouncementKind::AsyncState => 2,
    }
}

/// Announcement kind for a wire code.
#[must_use]
pub const fn notice_from_code(code: u8) -> Option<AnnouncementKind> {
    match code {
        1 => Some(AnnouncementKind::Focus),
        2 => Some(AnnouncementKind::AsyncState),
        _ => None,
    }
}

/// Role code for a fixed v1 role.
#[must_use]
pub const fn role_code(role: crate::Role) -> u8 {
    match role {
        crate::Role::Text => ROLE_TEXT,
        crate::Role::Group => ROLE_GROUP,
        crate::Role::Code => ROLE_CODE,
        crate::Role::Table => ROLE_TABLE,
        crate::Role::List => ROLE_LIST,
        crate::Role::Button => ROLE_BUTTON,
        crate::Role::Input => ROLE_INPUT,
        crate::Role::Separator => ROLE_SEPARATOR,
        crate::Role::Image => ROLE_IMAGE,
    }
}

/// Fixed v1 role for a wire role code; [`ROLE_NONE`] returns `None`.
#[must_use]
pub const fn role_from_code(code: u8) -> Option<crate::Role> {
    match code {
        ROLE_TEXT => Some(crate::Role::Text),
        ROLE_GROUP => Some(crate::Role::Group),
        ROLE_CODE => Some(crate::Role::Code),
        ROLE_TABLE => Some(crate::Role::Table),
        ROLE_LIST => Some(crate::Role::List),
        ROLE_BUTTON => Some(crate::Role::Button),
        ROLE_INPUT => Some(crate::Role::Input),
        ROLE_SEPARATOR => Some(crate::Role::Separator),
        ROLE_IMAGE => Some(crate::Role::Image),
        _ => None,
    }
}

/// Typed error category carried by `Error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The host withheld the capability or declared safe mode (code 1; host
    /// policy, enforced here as a refusal).
    Denied,
    /// A request was malformed or violated a bound (code 2).
    InvalidRequest,
    /// A capacity or byte budget is exhausted (code 3).
    Budget,
    /// The operation is not supported in v1 (code 4).
    Unsupported,
    /// The generation or element id is stale (code 5).
    StaleGeneration,
    /// The peer violated the protocol (code 6).
    Protocol,
    /// The component process was lost (code 7; produced by the host).
    ComponentLost,
    /// A host request deadline expired (code 8; produced by the host).
    Timeout,
    /// Internal component failure (code 9).
    Internal,
    /// The request rate gate rejected the exchange (code 10).
    RateLimited,
    /// No projection has been published yet (code 11).
    Unavailable,
}

impl ErrorKind {
    /// Wire code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Denied => 1,
            Self::InvalidRequest => 2,
            Self::Budget => 3,
            Self::Unsupported => 4,
            Self::StaleGeneration => 5,
            Self::Protocol => 6,
            Self::ComponentLost => 7,
            Self::Timeout => 8,
            Self::Internal => 9,
            Self::RateLimited => 10,
            Self::Unavailable => 11,
        }
    }

    /// Kind for a wire code, or `None` for an unknown code.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Denied),
            2 => Some(Self::InvalidRequest),
            3 => Some(Self::Budget),
            4 => Some(Self::Unsupported),
            5 => Some(Self::StaleGeneration),
            6 => Some(Self::Protocol),
            7 => Some(Self::ComponentLost),
            8 => Some(Self::Timeout),
            9 => Some(Self::Internal),
            10 => Some(Self::RateLimited),
            11 => Some(Self::Unavailable),
            _ => None,
        }
    }

    /// Stable `snake_case` name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::InvalidRequest => "invalid_request",
            Self::Budget => "budget",
            Self::Unsupported => "unsupported",
            Self::StaleGeneration => "stale_generation",
            Self::Protocol => "protocol",
            Self::ComponentLost => "component_lost",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "unavailable",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── messages ────────────────────────────────────────────────────────────────

/// One protocol v1 message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Handshake offer; must be the first frame (host -> component).
    Hello {
        /// Lowest protocol version the host supports.
        min: u16,
        /// Highest protocol version the host supports.
        max: u16,
        /// Component name the host expects.
        component: String,
        /// Host version string.
        version: String,
        /// Safe-mode/capability declaration bits.
        mode: u8,
    },
    /// Handshake answer (component -> host).
    HelloAck {
        /// Negotiated protocol version.
        protocol: u16,
        /// Component name.
        component: String,
        /// Component version.
        version: String,
        /// Capability bits available for this session; zero when deprived.
        capabilities: u8,
    },
    /// Start one bounded publication (host -> component).
    PublishBegin {
        /// Non-zero request id for this publication.
        request: u64,
        /// Projection generation.
        generation: u64,
        /// Window-scoped root accessible name.
        root_name: String,
        /// Terminal region name.
        title: String,
        /// Cursor row.
        cursor_row: u32,
        /// Cursor column.
        cursor_col: u32,
        /// Whether the cursor is rendered.
        cursor_visible: bool,
        /// Number of terminal rows that follow.
        terminal_rows: u32,
        /// Number of chrome entries that follow.
        chrome: u32,
        /// Number of scene entries that follow.
        scene: u32,
        /// Number of interactive entries that follow.
        interactive: u32,
        /// Number of announcement entries that follow.
        notices: u32,
        /// Whether a modal overlay is active.
        modal_overlay: bool,
        /// Accessible name of the focused surface, if any.
        focus: Option<String>,
        /// Host-declared selection, if any.
        selection: Option<Selection>,
    },
    /// One staged publication entry (host -> component).
    PublishNode {
        /// Request id from `PublishBegin`.
        request: u64,
        /// Entry to stage.
        node: PublishNode,
    },
    /// Finish and validate a publication (host -> component).
    PublishCommit {
        /// Request id from `PublishBegin`.
        request: u64,
        /// Projection generation from `PublishBegin`.
        generation: u64,
    },
    /// Ask for a snapshot or a delta (host -> component).
    SnapshotRequest {
        /// Non-zero request id for this exchange.
        request: u64,
        /// Generation the host already has; `0` asks for a full snapshot, an
        /// unknown or superseded generation also yields a full snapshot.
        since: u64,
    },
    /// Acknowledge the live generation after applying a diff (host ->
    /// component).
    DiffAck {
        /// Generation the host has fully applied.
        generation: u64,
    },
    /// Stop serving and exit (host -> component).
    Shutdown,
    /// Publication accepted and swapped in (component -> host).
    Published {
        /// Request id from `PublishBegin`.
        request: u64,
        /// Swapped-in generation.
        generation: u64,
        /// Nodes in the live projection.
        nodes: u32,
        /// Rows added relative to the acknowledged baseline.
        added: u32,
        /// Rows whose content changed relative to the baseline.
        updated: u32,
        /// Rows removed relative to the baseline.
        removed: u32,
    },
    /// First frame of a full snapshot (component -> host).
    SnapshotBegin {
        /// Request id from `SnapshotRequest`.
        request: u64,
        /// Live generation.
        generation: u64,
        /// Number of rows across all pages.
        nodes: u32,
        /// Stable id of the focused element, if any.
        focus: Option<u64>,
        /// Live selection, if any.
        selection: Option<Selection>,
        /// Page count; at least one.
        pages: u32,
    },
    /// One page of a full snapshot (component -> host).
    SnapshotPage {
        /// Request id from `SnapshotRequest`.
        request: u64,
        /// Zero-based page index.
        index: u32,
        /// Rows in this page (bounded by [`MAX_SNAPSHOT_PAGE_ROWS`]).
        rows: Vec<TreeRow>,
        /// Whether this is the last page.
        last: bool,
    },
    /// First frame of a bounded diff (component -> host).
    DiffBegin {
        /// Request id from `SnapshotRequest`.
        request: u64,
        /// Generation the diff applies to.
        base: u64,
        /// Live generation.
        generation: u64,
        /// Removed id count across all pages.
        removed: u32,
        /// Upserted row count across all pages.
        upserts: u32,
        /// Stable id of the focused element, if any.
        focus: Option<u64>,
        /// Live selection, if any.
        selection: Option<Selection>,
        /// Page count; `0` for an empty diff.
        pages: u32,
    },
    /// One page of a bounded diff (component -> host).
    DiffPage {
        /// Request id from `SnapshotRequest`.
        request: u64,
        /// Zero-based page index.
        index: u32,
        /// Ids no longer present in the live projection.
        removed: Vec<u64>,
        /// Rows added or changed in the live projection.
        upserts: Vec<TreeRow>,
        /// Whether this is the last page.
        last: bool,
    },
    /// Typed error (component -> host).
    Error {
        /// Request id, or [`CONNECTION_ID`] for a connection-level failure.
        request: u64,
        /// Error category.
        kind: ErrorKind,
        /// Bounded static diagnostic; never echoes names, text, or paths.
        message: String,
    },
    /// Shutdown completed; the component exits after this frame
    /// (component -> host).
    ShutdownAck,
}

impl Message {
    /// Wire tag of this message.
    #[must_use]
    pub const fn tag(&self) -> u8 {
        match self {
            Self::Hello { .. } => TAG_HELLO,
            Self::HelloAck { .. } => TAG_HELLO_ACK,
            Self::PublishBegin { .. } => TAG_PUBLISH_BEGIN,
            Self::PublishNode { .. } => TAG_PUBLISH_NODE,
            Self::PublishCommit { .. } => TAG_PUBLISH_COMMIT,
            Self::SnapshotRequest { .. } => TAG_SNAPSHOT_REQUEST,
            Self::DiffAck { .. } => TAG_DIFF_ACK,
            Self::Shutdown => TAG_SHUTDOWN,
            Self::Published { .. } => TAG_PUBLISHED,
            Self::SnapshotBegin { .. } => TAG_SNAPSHOT_BEGIN,
            Self::SnapshotPage { .. } => TAG_SNAPSHOT_PAGE,
            Self::DiffBegin { .. } => TAG_DIFF_BEGIN,
            Self::DiffPage { .. } => TAG_DIFF_PAGE,
            Self::Error { .. } => TAG_ERROR,
            Self::ShutdownAck => TAG_SHUTDOWN_ACK,
        }
    }

    /// Request id this message belongs to, when it carries one.
    #[must_use]
    pub const fn request_id(&self) -> Option<u64> {
        match self {
            Self::PublishBegin { request, .. }
            | Self::PublishNode { request, .. }
            | Self::PublishCommit { request, .. }
            | Self::SnapshotRequest { request, .. }
            | Self::Published { request, .. }
            | Self::SnapshotBegin { request, .. }
            | Self::SnapshotPage { request, .. }
            | Self::DiffBegin { request, .. }
            | Self::DiffPage { request, .. }
            | Self::Error { request, .. } => Some(*request),
            Self::Hello { .. }
            | Self::HelloAck { .. }
            | Self::DiffAck { .. }
            | Self::Shutdown
            | Self::ShutdownAck => None,
        }
    }
}

// ── errors ──────────────────────────────────────────────────────────────────

/// Wire codec failure (owned; no `io::Error` escapes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The underlying stream failed.
    Io(io::ErrorKind),
    /// A frame exceeds [`MAX_FRAME_BYTES`] (or an encoder field exceeds its
    /// bound).
    TooLarge {
        /// Observed length.
        len: usize,
    },
    /// A zero-length frame.
    EmptyFrame,
    /// A frame ended before a field was complete.
    Truncated,
    /// A frame carried bytes after the last field.
    TrailingBytes,
    /// The tag is not assigned in protocol v1.
    UnknownTag(u8),
    /// A string field is not UTF-8.
    InvalidUtf8,
    /// A field violates its grammar or bound.
    InvalidField(&'static str),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(kind) => write!(f, "stream i/o error: {kind}"),
            Self::TooLarge { len } => {
                write!(f, "frame is {len} bytes (limit {MAX_FRAME_BYTES})")
            }
            Self::EmptyFrame => f.write_str("empty frame"),
            Self::Truncated => f.write_str("truncated frame"),
            Self::TrailingBytes => f.write_str("trailing bytes after the last field"),
            Self::UnknownTag(tag) => write!(f, "unknown tag 0x{tag:02x}"),
            Self::InvalidUtf8 => f.write_str("string field is not UTF-8"),
            Self::InvalidField(reason) => write!(f, "invalid field: {reason}"),
        }
    }
}

impl std::error::Error for WireError {}

// ── framing ─────────────────────────────────────────────────────────────────

/// Reads length-prefixed frames from a stream.
pub struct FrameReader<R> {
    reader: R,
}

impl<R: Read> FrameReader<R> {
    /// Wraps `reader`.
    pub const fn new(reader: R) -> Self {
        Self { reader }
    }

    /// Reads one frame payload, or `None` at a clean EOF between frames.
    ///
    /// # Errors
    ///
    /// Returns [`WireError::Io`] for a stream failure,
    /// [`WireError::Truncated`] for EOF inside a header or payload,
    /// [`WireError::EmptyFrame`] for a zero length, and
    /// [`WireError::TooLarge`] for a length above [`MAX_FRAME_BYTES`]
    /// (detected before the payload is read or allocated).
    pub fn read_frame(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        let Some(len) = read_length(&mut self.reader)? else {
            return Ok(None);
        };
        if len == 0 {
            return Err(WireError::EmptyFrame);
        }
        if len > MAX_FRAME_BYTES {
            return Err(WireError::TooLarge { len });
        }
        let mut payload = vec![0_u8; len];
        self.reader
            .read_exact(&mut payload)
            .map_err(|error| match error.kind() {
                io::ErrorKind::UnexpectedEof => WireError::Truncated,
                kind => WireError::Io(kind),
            })?;
        Ok(Some(payload))
    }

    /// Reads and decodes one message, or `None` at a clean EOF.
    ///
    /// # Errors
    ///
    /// Returns the framing failures of [`Self::read_frame`] and the decode
    /// failures of [`decode_message`].
    pub fn read_message(&mut self) -> Result<Option<Message>, WireError> {
        match self.read_frame()? {
            Some(payload) => decode_message(&payload).map(Some),
            None => Ok(None),
        }
    }
}

fn read_length<R: Read>(reader: &mut R) -> Result<Option<usize>, WireError> {
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    let mut filled = 0;
    while filled < FRAME_HEADER_BYTES {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(WireError::Truncated),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(WireError::Io(error.kind())),
        }
    }
    Ok(Some(
        usize::try_from(u32::from_be_bytes(header)).unwrap_or(usize::MAX),
    ))
}

/// Encodes a message into one length-prefixed frame.
///
/// # Errors
///
/// Returns [`WireError::TooLarge`] when the payload would exceed
/// [`MAX_FRAME_BYTES`], or any field bound failure from [`encode_message`].
pub fn encode_frame(message: &Message) -> Result<Vec<u8>, WireError> {
    let payload = encode_message(message)?;
    let length =
        u32::try_from(payload.len()).map_err(|_| WireError::TooLarge { len: payload.len() })?;
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(WireError::TooLarge { len: payload.len() });
    }
    let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Encodes one frame and writes it to `writer`.
///
/// # Errors
///
/// Returns [`WireError::TooLarge`] for an over-bound message and
/// [`WireError::Io`] for a stream failure.
pub fn write_frame<W: Write>(writer: &mut W, message: &Message) -> Result<(), WireError> {
    let frame = encode_frame(message)?;
    writer
        .write_all(&frame)
        .map_err(|error| WireError::Io(error.kind()))
}

/// Encodes a message payload (tag and fields, no length prefix).
///
/// # Errors
///
/// Returns [`WireError::TooLarge`] when the payload would exceed
/// [`MAX_FRAME_BYTES`] and [`WireError::InvalidField`] when a field violates
/// its bound.
#[allow(clippy::too_many_lines)]
pub fn encode_message(message: &Message) -> Result<Vec<u8>, WireError> {
    let mut enc = Encoder::new();
    match message {
        Message::Hello {
            min,
            max,
            component,
            version,
            mode,
        } => {
            enc.u8(TAG_HELLO);
            enc.u16(*min);
            enc.u16(*max);
            enc.component_name(component)?;
            enc.version(version)?;
            enc.u8(*mode);
        }
        Message::HelloAck {
            protocol,
            component,
            version,
            capabilities,
        } => {
            enc.u8(TAG_HELLO_ACK);
            enc.u16(*protocol);
            enc.component_name(component)?;
            enc.version(version)?;
            if *capabilities & !CAPABILITY_MASK != 0 {
                return Err(WireError::InvalidField("capability bits"));
            }
            enc.u8(*capabilities);
        }
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
            require_request(*request)?;
            check_publish_counts(*terminal_rows, *chrome, *scene, *interactive, *notices)?;
            check_cursor(*cursor_row, *cursor_col)?;
            enc.u8(TAG_PUBLISH_BEGIN);
            enc.u64(*request);
            enc.u64(*generation);
            enc.name(root_name)?;
            enc.name(title)?;
            enc.u32(*cursor_row);
            enc.u32(*cursor_col);
            enc.boolean(*cursor_visible);
            enc.u32(*terminal_rows);
            enc.u32(*chrome);
            enc.u32(*scene);
            enc.u32(*interactive);
            enc.u32(*notices);
            enc.boolean(*modal_overlay);
            enc.opt_name(focus.as_deref())?;
            enc.opt_selection(*selection);
        }
        Message::PublishNode { request, node } => {
            require_request(*request)?;
            enc.u8(TAG_PUBLISH_NODE);
            enc.u64(*request);
            enc.u8(node.code());
            match node {
                PublishNode::TerminalRow { row, text } => {
                    enc.u32(*row);
                    enc.text(text)?;
                }
                PublishNode::Chrome { kind, name, state } => {
                    enc.u8(chrome_code(*kind));
                    enc.name(name)?;
                    enc.opt_name(state.as_deref())?;
                }
                PublishNode::Scene { kind, identity } => {
                    enc.u8(scene_code(*kind));
                    enc.u64(*identity);
                }
                PublishNode::Interactive {
                    purpose,
                    name,
                    enabled,
                } => {
                    enc.u8(purpose_code(*purpose));
                    enc.name(name)?;
                    enc.boolean(*enabled);
                }
                PublishNode::Notice { kind, text } => {
                    enc.u8(notice_code(*kind));
                    enc.name(text)?;
                }
            }
        }
        Message::PublishCommit {
            request,
            generation,
        } => {
            require_request(*request)?;
            enc.u8(TAG_PUBLISH_COMMIT);
            enc.u64(*request);
            enc.u64(*generation);
        }
        Message::SnapshotRequest { request, since } => {
            require_request(*request)?;
            enc.u8(TAG_SNAPSHOT_REQUEST);
            enc.u64(*request);
            enc.u64(*since);
        }
        Message::DiffAck { generation } => {
            enc.u8(TAG_DIFF_ACK);
            enc.u64(*generation);
        }
        Message::Shutdown => enc.u8(TAG_SHUTDOWN),
        Message::Published {
            request,
            generation,
            nodes,
            added,
            updated,
            removed,
        } => {
            require_request(*request)?;
            check_node_count(*nodes)?;
            check_delta_count(*added)?;
            check_delta_count(*updated)?;
            check_delta_count(*removed)?;
            enc.u8(TAG_PUBLISHED);
            enc.u64(*request);
            enc.u64(*generation);
            enc.u32(*nodes);
            enc.u32(*added);
            enc.u32(*updated);
            enc.u32(*removed);
        }
        Message::SnapshotBegin {
            request,
            generation,
            nodes,
            focus,
            selection,
            pages,
        } => {
            require_request(*request)?;
            check_node_count(*nodes)?;
            let expected = page_count(*nodes as usize, MAX_SNAPSHOT_PAGE_ROWS);
            if *nodes == 0 || *pages != expected {
                return Err(WireError::InvalidField("snapshot page count"));
            }
            enc.u8(TAG_SNAPSHOT_BEGIN);
            enc.u64(*request);
            enc.u64(*generation);
            enc.u32(*nodes);
            enc.opt_id(*focus);
            enc.opt_selection(*selection);
            enc.u32(*pages);
        }
        Message::SnapshotPage {
            request,
            index,
            rows,
            last,
        } => {
            require_request(*request)?;
            check_page_index(*index)?;
            if rows.len() > MAX_SNAPSHOT_PAGE_ROWS {
                return Err(WireError::InvalidField("snapshot page rows"));
            }
            if rows.is_empty() {
                return Err(WireError::InvalidField("empty snapshot page"));
            }
            enc.u8(TAG_SNAPSHOT_PAGE);
            enc.u64(*request);
            enc.u32(*index);
            enc.rows(rows)?;
            enc.boolean(*last);
        }
        Message::DiffBegin {
            request,
            base,
            generation,
            removed,
            upserts,
            focus,
            selection,
            pages,
        } => {
            require_request(*request)?;
            check_node_count(*removed)?;
            check_node_count(*upserts)?;
            let expected = diff_page_count(*removed as usize, *upserts as usize);
            if *pages != expected {
                return Err(WireError::InvalidField("diff page count"));
            }
            enc.u8(TAG_DIFF_BEGIN);
            enc.u64(*request);
            enc.u64(*base);
            enc.u64(*generation);
            enc.u32(*removed);
            enc.u32(*upserts);
            enc.opt_id(*focus);
            enc.opt_selection(*selection);
            enc.u32(*pages);
        }
        Message::DiffPage {
            request,
            index,
            removed,
            upserts,
            last,
        } => {
            require_request(*request)?;
            check_page_index(*index)?;
            if removed.len() > MAX_DIFF_PAGE_REMOVED || upserts.len() > MAX_DIFF_PAGE_ROWS {
                return Err(WireError::InvalidField("diff page size"));
            }
            if removed.is_empty() && upserts.is_empty() {
                return Err(WireError::InvalidField("empty diff page"));
            }
            enc.u8(TAG_DIFF_PAGE);
            enc.u64(*request);
            enc.u32(*index);
            enc.u32(
                u32::try_from(removed.len())
                    .map_err(|_| WireError::InvalidField("removed count"))?,
            );
            for id in removed {
                if *id == 0 {
                    return Err(WireError::InvalidField("zero removed id"));
                }
                enc.u64(*id);
            }
            enc.rows(upserts)?;
            enc.boolean(*last);
        }
        Message::Error {
            request,
            kind,
            message,
        } => {
            enc.u8(TAG_ERROR);
            enc.u64(*request);
            enc.u8(kind.code());
            enc.error_message(message)?;
        }
        Message::ShutdownAck => enc.u8(TAG_SHUTDOWN_ACK),
    }
    if enc.buf.len() > MAX_FRAME_BYTES {
        return Err(WireError::TooLarge { len: enc.buf.len() });
    }
    Ok(enc.buf)
}

/// Decodes one payload (tag and fields, no length prefix).
///
/// # Errors
///
/// Returns [`WireError`] for an empty or oversize payload, unknown tags,
/// truncated fields, trailing bytes, invalid UTF-8, invalid booleans, or
/// any violated field bound.
#[allow(clippy::too_many_lines)]
pub fn decode_message(payload: &[u8]) -> Result<Message, WireError> {
    if payload.is_empty() {
        return Err(WireError::EmptyFrame);
    }
    if payload.len() > MAX_FRAME_BYTES {
        return Err(WireError::TooLarge { len: payload.len() });
    }
    let mut dec = Decoder::new(payload);
    let tag = dec.u8()?;
    let message = match tag {
        TAG_HELLO => Message::Hello {
            min: dec.u16()?,
            max: dec.u16()?,
            component: dec.component_name()?,
            version: dec.version()?,
            mode: dec.u8()?,
        },
        TAG_HELLO_ACK => {
            let protocol = dec.u16()?;
            let component = dec.component_name()?;
            let version = dec.version()?;
            let capabilities = dec.u8()?;
            if capabilities & !CAPABILITY_MASK != 0 {
                return Err(WireError::InvalidField("capability bits"));
            }
            Message::HelloAck {
                protocol,
                component,
                version,
                capabilities,
            }
        }
        TAG_PUBLISH_BEGIN => Message::PublishBegin {
            request: dec.request()?,
            generation: dec.u64()?,
            root_name: dec.name()?,
            title: dec.name()?,
            cursor_row: dec.u32()?,
            cursor_col: dec.u32()?,
            cursor_visible: dec.boolean()?,
            terminal_rows: dec.u32()?,
            chrome: dec.u32()?,
            scene: dec.u32()?,
            interactive: dec.u32()?,
            notices: dec.u32()?,
            modal_overlay: dec.boolean()?,
            focus: dec.opt_name()?,
            selection: dec.opt_selection()?,
        },
        TAG_PUBLISH_NODE => {
            let request = dec.request()?;
            let node_tag = dec.u8()?;
            let node = match node_tag {
                PublishNode::TAG_TERMINAL_ROW => PublishNode::TerminalRow {
                    row: dec.u32()?,
                    text: dec.text()?,
                },
                PublishNode::TAG_CHROME => PublishNode::Chrome {
                    kind: dec.chrome()?,
                    name: dec.name()?,
                    state: dec.opt_name()?,
                },
                PublishNode::TAG_SCENE => PublishNode::Scene {
                    kind: dec.scene()?,
                    identity: dec.u64()?,
                },
                PublishNode::TAG_INTERACTIVE => PublishNode::Interactive {
                    purpose: dec.purpose()?,
                    name: dec.name()?,
                    enabled: dec.boolean()?,
                },
                PublishNode::TAG_NOTICE => PublishNode::Notice {
                    kind: dec.notice()?,
                    text: dec.name()?,
                },
                _ => return Err(WireError::InvalidField("publish node kind")),
            };
            Message::PublishNode { request, node }
        }
        TAG_PUBLISH_COMMIT => Message::PublishCommit {
            request: dec.request()?,
            generation: dec.u64()?,
        },
        TAG_SNAPSHOT_REQUEST => Message::SnapshotRequest {
            request: dec.request()?,
            since: dec.u64()?,
        },
        TAG_DIFF_ACK => Message::DiffAck {
            generation: dec.u64()?,
        },
        TAG_SHUTDOWN => Message::Shutdown,
        TAG_PUBLISHED => Message::Published {
            request: dec.request()?,
            generation: dec.u64()?,
            nodes: dec.u32()?,
            added: dec.u32()?,
            updated: dec.u32()?,
            removed: dec.u32()?,
        },
        TAG_SNAPSHOT_BEGIN => Message::SnapshotBegin {
            request: dec.request()?,
            generation: dec.u64()?,
            nodes: dec.u32()?,
            focus: dec.opt_id()?,
            selection: dec.opt_selection()?,
            pages: dec.u32()?,
        },
        TAG_SNAPSHOT_PAGE => {
            let request = dec.request()?;
            let index = dec.u32()?;
            let rows = dec.rows()?;
            let last = dec.boolean()?;
            Message::SnapshotPage {
                request,
                index,
                rows,
                last,
            }
        }
        TAG_DIFF_BEGIN => Message::DiffBegin {
            request: dec.request()?,
            base: dec.u64()?,
            generation: dec.u64()?,
            removed: dec.u32()?,
            upserts: dec.u32()?,
            focus: dec.opt_id()?,
            selection: dec.opt_selection()?,
            pages: dec.u32()?,
        },
        TAG_DIFF_PAGE => {
            let request = dec.request()?;
            let index = dec.u32()?;
            let removed_count = usize::try_from(dec.u32()?)
                .map_err(|_| WireError::InvalidField("removed count"))?;
            if removed_count > MAX_DIFF_PAGE_REMOVED {
                return Err(WireError::InvalidField("removed count"));
            }
            let mut removed = Vec::with_capacity(removed_count);
            for _ in 0..removed_count {
                let id = dec.u64()?;
                if id == 0 {
                    return Err(WireError::InvalidField("zero removed id"));
                }
                removed.push(id);
            }
            let upserts = dec.rows()?;
            let last = dec.boolean()?;
            Message::DiffPage {
                request,
                index,
                removed,
                upserts,
                last,
            }
        }
        TAG_ERROR => Message::Error {
            request: dec.u64()?,
            kind: ErrorKind::from_code(dec.u8()?).ok_or(WireError::InvalidField("error kind"))?,
            message: dec.error_message()?,
        },
        TAG_SHUTDOWN_ACK => Message::ShutdownAck,
        other => return Err(WireError::UnknownTag(other)),
    };
    if !dec.is_empty() {
        return Err(WireError::TrailingBytes);
    }
    validate_message(&message)?;
    Ok(message)
}

/// Validates cross-field invariants that are not visible to the field
/// decoder.
fn validate_message(message: &Message) -> Result<(), WireError> {
    match message {
        Message::PublishBegin {
            cursor_row,
            cursor_col,
            terminal_rows,
            chrome,
            scene,
            interactive,
            notices,
            ..
        } => {
            check_cursor(*cursor_row, *cursor_col)?;
            check_publish_counts(*terminal_rows, *chrome, *scene, *interactive, *notices)?;
        }
        Message::Published {
            nodes,
            added,
            updated,
            removed,
            ..
        } => {
            check_node_count(*nodes)?;
            check_delta_count(*added)?;
            check_delta_count(*updated)?;
            check_delta_count(*removed)?;
        }
        Message::SnapshotBegin { nodes, pages, .. } => {
            check_node_count(*nodes)?;
            if *nodes == 0 || *pages != page_count(*nodes as usize, MAX_SNAPSHOT_PAGE_ROWS) {
                return Err(WireError::InvalidField("snapshot page count"));
            }
        }
        Message::SnapshotPage { rows, .. } => {
            if rows.is_empty() || rows.len() > MAX_SNAPSHOT_PAGE_ROWS {
                return Err(WireError::InvalidField("snapshot page rows"));
            }
        }
        Message::DiffBegin {
            removed,
            upserts,
            pages,
            ..
        } => {
            check_node_count(*removed)?;
            check_node_count(*upserts)?;
            if *pages != diff_page_count(*removed as usize, *upserts as usize) {
                return Err(WireError::InvalidField("diff page count"));
            }
        }
        Message::DiffPage {
            removed, upserts, ..
        } => {
            if removed.is_empty() && upserts.is_empty() {
                return Err(WireError::InvalidField("empty diff page"));
            }
            if removed.len() > MAX_DIFF_PAGE_REMOVED || upserts.len() > MAX_DIFF_PAGE_ROWS {
                return Err(WireError::InvalidField("diff page size"));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Intersects the host's protocol range with this component's range.
#[must_use]
pub fn negotiate(min: u16, max: u16) -> Option<u16> {
    if min > max {
        return None;
    }
    let low = min.max(PROTOCOL_VERSION);
    let high = max.min(PROTOCOL_VERSION);
    (low <= high).then_some(PROTOCOL_VERSION)
}

/// Component name this protocol expects in `Hello`.
#[must_use]
pub fn expected_component_name() -> &'static str {
    super::descriptor::COMPONENT_NAME
}

/// Protocol range this component supports.
#[must_use]
pub const fn supported_protocol_range() -> (u16, u16) {
    (
        super::descriptor::COMPONENT_PROTOCOL_MIN,
        super::descriptor::COMPONENT_PROTOCOL_MAX,
    )
}

fn require_request(request: u64) -> Result<(), WireError> {
    if request == CONNECTION_ID {
        return Err(WireError::InvalidField("zero request id"));
    }
    Ok(())
}

fn check_name(value: &str) -> Result<(), WireError> {
    if value.chars().count() > MAX_ACCESSIBLE_NAME_LEN || value.len() > MAX_NAME_BYTES {
        return Err(WireError::InvalidField("name length"));
    }
    Ok(())
}

fn check_text(value: &str) -> Result<(), WireError> {
    if value.len() > MAX_ROW_TEXT_BYTES {
        return Err(WireError::InvalidField("row text length"));
    }
    Ok(())
}

fn check_version(value: &str) -> Result<(), WireError> {
    if value.len() > MAX_VERSION_BYTES || value.chars().any(char::is_control) {
        return Err(WireError::InvalidField("version"));
    }
    Ok(())
}

fn check_component_name(value: &str) -> Result<(), WireError> {
    let bytes = value.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= MAX_COMPONENT_NAME_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    if valid {
        Ok(())
    } else {
        Err(WireError::InvalidField("component name"))
    }
}

fn check_error_message(value: &str) -> Result<(), WireError> {
    if value.is_empty()
        || value.len() > MAX_ERROR_MESSAGE_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(WireError::InvalidField("error message"));
    }
    Ok(())
}

fn check_cursor(row: u32, col: u32) -> Result<(), WireError> {
    if row >= MAX_GRID_ROWS || col >= MAX_GRID_COLS {
        return Err(WireError::InvalidField("cursor coordinate"));
    }
    Ok(())
}

fn check_selection(selection: Selection) -> Result<(), WireError> {
    for (row, col) in [
        (selection.anchor_row, selection.anchor_col),
        (selection.focus_row, selection.focus_col),
    ] {
        if row >= MAX_GRID_ROWS || col >= MAX_GRID_COLS {
            return Err(WireError::InvalidField("selection coordinate"));
        }
    }
    Ok(())
}

fn check_node_count(count: u32) -> Result<(), WireError> {
    if count as usize > MAX_SNAPSHOT_NODES {
        return Err(WireError::InvalidField("node count"));
    }
    Ok(())
}

fn check_delta_count(count: u32) -> Result<(), WireError> {
    if count as usize > MAX_SNAPSHOT_NODES {
        return Err(WireError::InvalidField("delta count"));
    }
    Ok(())
}

fn check_publish_counts(
    terminal_rows: u32,
    chrome: u32,
    scene: u32,
    interactive: u32,
    notices: u32,
) -> Result<(), WireError> {
    check_node_count(terminal_rows)?;
    check_node_count(chrome)?;
    check_node_count(scene)?;
    check_node_count(interactive)?;
    if notices as usize > MAX_PENDING_ANNOUNCEMENTS {
        return Err(WireError::InvalidField("notice count"));
    }
    let nodes = 2usize
        .saturating_add(terminal_rows as usize)
        .saturating_add(chrome as usize)
        .saturating_add(scene as usize)
        .saturating_add(interactive as usize);
    if nodes > MAX_SNAPSHOT_NODES {
        return Err(WireError::InvalidField("publication node count"));
    }
    if terminal_rows as usize
        + chrome as usize
        + scene as usize
        + interactive as usize
        + notices as usize
        > MAX_PUBLISH_ENTRIES
    {
        return Err(WireError::InvalidField("publication entry count"));
    }
    Ok(())
}

fn check_page_index(index: u32) -> Result<(), WireError> {
    if index >= MAX_PAGES {
        return Err(WireError::InvalidField("page index"));
    }
    Ok(())
}

/// Number of fixed-size pages needed for `items` entries.
#[must_use]
pub fn page_count(items: usize, page_size: usize) -> u32 {
    if items == 0 {
        return 0;
    }
    u32::try_from(items.div_ceil(page_size)).unwrap_or(u32::MAX)
}

fn diff_page_count(removed: usize, upserts: usize) -> u32 {
    page_count(removed, MAX_DIFF_PAGE_REMOVED).max(page_count(upserts, MAX_DIFF_PAGE_ROWS))
}

// ── encoder / decoder ───────────────────────────────────────────────────────

struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64),
        }
    }

    fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    fn u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    fn boolean(&mut self, value: bool) {
        self.buf.push(u8::from(value));
    }

    fn name(&mut self, value: &str) -> Result<(), WireError> {
        check_name(value)?;
        self.str(value)
    }

    fn opt_name(&mut self, value: Option<&str>) -> Result<(), WireError> {
        if let Some(value) = value {
            self.u8(1);
            self.name(value)
        } else {
            self.u8(0);
            Ok(())
        }
    }

    fn opt_id(&mut self, value: Option<u64>) {
        if let Some(value) = value.filter(|id| *id != 0) {
            self.u8(1);
            self.u64(value);
        } else {
            self.u8(0);
        }
    }

    fn opt_selection(&mut self, value: Option<Selection>) {
        if let Some(value) = value {
            self.u8(1);
            self.u32(value.anchor_row);
            self.u32(value.anchor_col);
            self.u32(value.focus_row);
            self.u32(value.focus_col);
        } else {
            self.u8(0);
        }
    }

    fn text(&mut self, value: &str) -> Result<(), WireError> {
        check_text(value)?;
        self.str(value)
    }

    fn component_name(&mut self, value: &str) -> Result<(), WireError> {
        check_component_name(value)?;
        self.str(value)
    }

    fn version(&mut self, value: &str) -> Result<(), WireError> {
        check_version(value)?;
        self.str(value)
    }

    fn error_message(&mut self, value: &str) -> Result<(), WireError> {
        check_error_message(value)?;
        self.str(value)
    }

    fn str(&mut self, value: &str) -> Result<(), WireError> {
        let len =
            u32::try_from(value.len()).map_err(|_| WireError::InvalidField("string length"))?;
        self.u32(len);
        self.buf.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn rows(&mut self, rows: &[TreeRow]) -> Result<(), WireError> {
        let count = u32::try_from(rows.len()).map_err(|_| WireError::InvalidField("row count"))?;
        self.u32(count);
        for row in rows {
            self.row(row)?;
        }
        Ok(())
    }

    fn row(&mut self, row: &TreeRow) -> Result<(), WireError> {
        if row.id == 0 || row.depth > MAX_TREE_DEPTH || row.states & !STATE_MASK != 0 {
            return Err(WireError::InvalidField("tree row"));
        }
        if row.parent == 0 && row.depth != 0 {
            return Err(WireError::InvalidField("tree row parent"));
        }
        if row.depth == 0 && row.parent != 0 {
            return Err(WireError::InvalidField("tree row root parent"));
        }
        if row.role > ROLE_NONE || row.surface > SURFACE_OVERLAY {
            return Err(WireError::InvalidField("tree row role"));
        }
        if (row.surface == SURFACE_NONE) != (row.role != ROLE_NONE) {
            return Err(WireError::InvalidField("tree row surface"));
        }
        if let Some(span) = row.selection
            && span.start_col >= span.end_col
        {
            return Err(WireError::InvalidField("selection span"));
        }
        if let Some(bounds) = row.bounds
            && bounds.unit != BOUNDS_UNIT_CHARS
        {
            return Err(WireError::InvalidField("bounds unit"));
        }
        self.u64(row.id);
        self.u64(row.parent);
        self.u8(row.depth);
        self.u8(row.role);
        self.u8(row.surface);
        self.opt_name(row.name.as_deref())?;
        match row.content.as_deref() {
            Some(content) => {
                self.u8(1);
                self.text(content)?;
            }
            None => self.u8(0),
        }
        self.opt_name(row.state_text.as_deref())?;
        self.u16(row.states);
        match row.focus {
            Some(focus) => {
                self.u8(1);
                self.u32(focus.row);
                self.u32(focus.col);
            }
            None => self.u8(0),
        }
        match row.selection {
            Some(span) => {
                self.u8(1);
                self.u32(span.start_col);
                self.u32(span.end_col);
            }
            None => self.u8(0),
        }
        match row.bounds {
            Some(bounds) => {
                self.u8(1);
                self.u32(bounds.x);
                self.u32(bounds.y);
                self.u32(bounds.width);
                self.u32(bounds.height);
                self.u8(bounds.unit);
            }
            None => self.u8(0),
        }
        Ok(())
    }
}

struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.buf.len()
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        let end = self.pos.checked_add(len).ok_or(WireError::Truncated)?;
        let slice = self.buf.get(self.pos..end).ok_or(WireError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        let bytes: [u8; 2] = self.take(2)?.try_into().map_err(|_| WireError::Truncated)?;
        Ok(u16::from_be_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().map_err(|_| WireError::Truncated)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, WireError> {
        let bytes: [u8; 8] = self.take(8)?.try_into().map_err(|_| WireError::Truncated)?;
        Ok(u64::from_be_bytes(bytes))
    }

    fn boolean(&mut self) -> Result<bool, WireError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WireError::InvalidField("boolean")),
        }
    }

    fn str(&mut self, max: usize) -> Result<String, WireError> {
        let len = usize::try_from(self.u32()?).map_err(|_| WireError::InvalidField("length"))?;
        if len > max {
            return Err(WireError::InvalidField("string length"));
        }
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| WireError::InvalidUtf8)
    }

    fn name(&mut self) -> Result<String, WireError> {
        let value = self.str(MAX_NAME_BYTES)?;
        check_name(&value)?;
        Ok(value)
    }

    fn opt_name(&mut self) -> Result<Option<String>, WireError> {
        match self.u8()? {
            0 => Ok(None),
            1 => self.name().map(Some),
            _ => Err(WireError::InvalidField("optional")),
        }
    }

    fn opt_id(&mut self) -> Result<Option<u64>, WireError> {
        match self.u8()? {
            0 => Ok(None),
            1 => {
                let id = self.u64()?;
                if id == 0 {
                    return Err(WireError::InvalidField("zero id"));
                }
                Ok(Some(id))
            }
            _ => Err(WireError::InvalidField("optional")),
        }
    }

    fn opt_selection(&mut self) -> Result<Option<Selection>, WireError> {
        match self.u8()? {
            0 => Ok(None),
            1 => {
                let selection = Selection {
                    anchor_row: self.u32()?,
                    anchor_col: self.u32()?,
                    focus_row: self.u32()?,
                    focus_col: self.u32()?,
                };
                check_selection(selection)?;
                Ok(Some(selection))
            }
            _ => Err(WireError::InvalidField("optional")),
        }
    }

    fn text(&mut self) -> Result<String, WireError> {
        let value = self.str(MAX_ROW_TEXT_BYTES)?;
        check_text(&value)?;
        Ok(value)
    }

    fn component_name(&mut self) -> Result<String, WireError> {
        let value = self.str(MAX_COMPONENT_NAME_BYTES)?;
        check_component_name(&value)?;
        Ok(value)
    }

    fn version(&mut self) -> Result<String, WireError> {
        let value = self.str(MAX_VERSION_BYTES)?;
        check_version(&value)?;
        Ok(value)
    }

    fn error_message(&mut self) -> Result<String, WireError> {
        let value = self.str(MAX_ERROR_MESSAGE_BYTES)?;
        check_error_message(&value)?;
        Ok(value)
    }

    fn request(&mut self) -> Result<u64, WireError> {
        let request = self.u64()?;
        require_request(request)?;
        Ok(request)
    }

    fn chrome(&mut self) -> Result<ChromeKind, WireError> {
        chrome_from_code(self.u8()?).ok_or(WireError::InvalidField("chrome kind"))
    }

    fn scene(&mut self) -> Result<SceneKind, WireError> {
        scene_from_code(self.u8()?).ok_or(WireError::InvalidField("scene kind"))
    }

    fn purpose(&mut self) -> Result<InteractivePurpose, WireError> {
        purpose_from_code(self.u8()?).ok_or(WireError::InvalidField("interactive purpose"))
    }

    fn notice(&mut self) -> Result<AnnouncementKind, WireError> {
        notice_from_code(self.u8()?).ok_or(WireError::InvalidField("notice kind"))
    }

    fn rows(&mut self) -> Result<Vec<TreeRow>, WireError> {
        let count =
            usize::try_from(self.u32()?).map_err(|_| WireError::InvalidField("row count"))?;
        if count > MAX_SNAPSHOT_PAGE_ROWS {
            return Err(WireError::InvalidField("row count"));
        }
        let mut rows = Vec::with_capacity(count);
        for _ in 0..count {
            rows.push(self.row()?);
        }
        Ok(rows)
    }

    fn row(&mut self) -> Result<TreeRow, WireError> {
        let id = self.u64()?;
        let parent = self.u64()?;
        let depth = self.u8()?;
        let role = self.u8()?;
        let surface = self.u8()?;
        let name = self.opt_name()?;
        let content = match self.u8()? {
            0 => None,
            1 => Some(self.text()?),
            _ => return Err(WireError::InvalidField("optional")),
        };
        let state_text = self.opt_name()?;
        let states = self.u16()?;
        let focus = match self.u8()? {
            0 => None,
            1 => Some(FocusCell {
                row: self.u32()?,
                col: self.u32()?,
            }),
            _ => return Err(WireError::InvalidField("optional")),
        };
        let selection = match self.u8()? {
            0 => None,
            1 => Some(SelectionSpan {
                start_col: self.u32()?,
                end_col: self.u32()?,
            }),
            _ => return Err(WireError::InvalidField("optional")),
        };
        let bounds = match self.u8()? {
            0 => None,
            1 => Some(Bounds {
                x: self.u32()?,
                y: self.u32()?,
                width: self.u32()?,
                height: self.u32()?,
                unit: self.u8()?,
            }),
            _ => return Err(WireError::InvalidField("optional")),
        };
        let row = TreeRow {
            id,
            parent,
            depth,
            role,
            surface,
            name,
            content,
            state_text,
            states,
            focus,
            selection,
            bounds,
        };
        if row.id == 0 || row.depth > MAX_TREE_DEPTH || row.states & !STATE_MASK != 0 {
            return Err(WireError::InvalidField("tree row"));
        }
        if row.parent == 0 && row.depth != 0 {
            return Err(WireError::InvalidField("tree row parent"));
        }
        if row.depth == 0 && row.parent != 0 {
            return Err(WireError::InvalidField("tree row root parent"));
        }
        if row.role > ROLE_NONE || row.surface > SURFACE_OVERLAY {
            return Err(WireError::InvalidField("tree row role"));
        }
        if (row.surface == SURFACE_NONE) != (row.role != ROLE_NONE) {
            return Err(WireError::InvalidField("tree row surface"));
        }
        if let Some(span) = row.selection
            && span.start_col >= span.end_col
        {
            return Err(WireError::InvalidField("selection span"));
        }
        if let Some(bounds) = row.bounds
            && bounds.unit != BOUNDS_UNIT_CHARS
        {
            return Err(WireError::InvalidField("bounds unit"));
        }
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u64) -> TreeRow {
        TreeRow {
            id,
            parent: 0,
            depth: 0,
            role: ROLE_GROUP,
            surface: SURFACE_NONE,
            name: Some("root".to_owned()),
            content: None,
            state_text: None,
            states: 0,
            focus: None,
            selection: None,
            bounds: None,
        }
    }

    fn round_trip(message: Message) {
        let payload = encode_message(&message).expect("encode");
        let decoded = decode_message(&payload).expect("decode");
        assert_eq!(decoded, message);
        let frame = encode_frame(&message).expect("frame");
        let mut reader = FrameReader::new(frame.as_slice());
        assert_eq!(reader.read_message().expect("read"), Some(message));
        assert_eq!(reader.read_message().expect("eof"), None);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn round_trips_every_message() {
        round_trip(Message::Hello {
            min: 1,
            max: 1,
            component: "a11y".to_owned(),
            version: "0.0.1".to_owned(),
            mode: HELLO_CAPABILITY_GRANTED,
        });
        round_trip(Message::HelloAck {
            protocol: 1,
            component: "a11y".to_owned(),
            version: "0.0.1".to_owned(),
            capabilities: CAPABILITY_MASK,
        });
        round_trip(Message::PublishBegin {
            request: 7,
            generation: 3,
            root_name: "window".to_owned(),
            title: "shell".to_owned(),
            cursor_row: 2,
            cursor_col: 4,
            cursor_visible: true,
            terminal_rows: 1,
            chrome: 1,
            scene: 1,
            interactive: 1,
            notices: 1,
            modal_overlay: true,
            focus: Some("shell".to_owned()),
            selection: Some(Selection {
                anchor_row: 0,
                anchor_col: 1,
                focus_row: 2,
                focus_col: 3,
            }),
        });
        round_trip(Message::PublishNode {
            request: 7,
            node: PublishNode::TerminalRow {
                row: 0,
                text: "$ ls".to_owned(),
            },
        });
        round_trip(Message::PublishNode {
            request: 7,
            node: PublishNode::Chrome {
                kind: ChromeKind::Overlay,
                name: "dialog".to_owned(),
                state: Some("open".to_owned()),
            },
        });
        round_trip(Message::PublishNode {
            request: 7,
            node: PublishNode::Scene {
                kind: SceneKind::Table,
                identity: 9,
            },
        });
        round_trip(Message::PublishNode {
            request: 7,
            node: PublishNode::Interactive {
                purpose: InteractivePurpose::Button,
                name: "ok".to_owned(),
                enabled: true,
            },
        });
        round_trip(Message::PublishNode {
            request: 7,
            node: PublishNode::Notice {
                kind: AnnouncementKind::AsyncState,
                text: "done".to_owned(),
            },
        });
        round_trip(Message::PublishCommit {
            request: 7,
            generation: 3,
        });
        round_trip(Message::SnapshotRequest {
            request: 8,
            since: 3,
        });
        round_trip(Message::DiffAck { generation: 3 });
        round_trip(Message::Shutdown);
        round_trip(Message::Published {
            request: 7,
            generation: 3,
            nodes: 2,
            added: 1,
            updated: 0,
            removed: 0,
        });
        round_trip(Message::SnapshotBegin {
            request: 8,
            generation: 3,
            nodes: 2,
            focus: Some(1),
            selection: None,
            pages: 1,
        });
        round_trip(Message::SnapshotPage {
            request: 8,
            index: 0,
            rows: vec![row(1), row(2)],
            last: true,
        });
        round_trip(Message::DiffBegin {
            request: 8,
            base: 2,
            generation: 3,
            removed: 1,
            upserts: 1,
            focus: None,
            selection: None,
            pages: 1,
        });
        round_trip(Message::DiffPage {
            request: 8,
            index: 0,
            removed: vec![9],
            upserts: vec![row(1)],
            last: true,
        });
        round_trip(Message::Error {
            request: 0,
            kind: ErrorKind::Protocol,
            message: "handshake required".to_owned(),
        });
        round_trip(Message::ShutdownAck);
    }

    #[test]
    fn rejects_malformed_payloads() {
        assert_eq!(decode_message(&[]), Err(WireError::EmptyFrame));
        assert_eq!(decode_message(&[0xEE]), Err(WireError::UnknownTag(0xEE)));
        assert_eq!(decode_message(&[TAG_HELLO]), Err(WireError::Truncated));
        let mut trailing = encode_message(&Message::Shutdown).unwrap_or_default();
        trailing.push(0);
        assert_eq!(decode_message(&trailing), Err(WireError::TrailingBytes));
        assert_eq!(
            decode_message(&[TAG_HELLO, 0, 1, 0, 1, 0, 0, 0, 1, b'x'][..]),
            Err(WireError::Truncated)
        );
        assert_eq!(
            decode_message(&[TAG_HELLO, 0, 1, 0, 1, 0, 0, 0, 1, b'A', 0, 0, 0, 1, b'v', 2,][..]),
            Err(WireError::InvalidField("component name"))
        );
    }

    #[test]
    fn rejects_zero_request_ids() {
        assert_eq!(
            encode_message(&Message::SnapshotRequest {
                request: 0,
                since: 0,
            }),
            Err(WireError::InvalidField("zero request id"))
        );
        let mut payload = encode_message(&Message::SnapshotRequest {
            request: 1,
            since: 0,
        })
        .expect("encode");
        payload[1..9].copy_from_slice(&0_u64.to_be_bytes());
        assert_eq!(
            decode_message(&payload),
            Err(WireError::InvalidField("zero request id"))
        );
    }

    #[test]
    fn frame_reader_rejects_oversize_before_payload() {
        let header = u32::try_from(MAX_FRAME_BYTES + 1)
            .unwrap_or(u32::MAX)
            .to_be_bytes();
        let mut reader = FrameReader::new(header.as_slice());
        assert!(matches!(
            reader.read_frame(),
            Err(WireError::TooLarge { .. })
        ));
    }

    #[test]
    fn negotiate_is_intersection_only() {
        assert_eq!(negotiate(1, 1), Some(1));
        assert_eq!(negotiate(0, 0), None);
        assert_eq!(negotiate(2, 3), None);
        assert_eq!(negotiate(0, 9), Some(1));
    }

    #[test]
    fn maximal_page_stays_under_the_frame_cap() {
        let name = "n".repeat(MAX_ACCESSIBLE_NAME_LEN);
        let text = "t".repeat(MAX_ROW_TEXT_BYTES);
        let state = "s".repeat(MAX_ACCESSIBLE_NAME_LEN);
        let row = TreeRow {
            id: 1,
            parent: 0,
            depth: 0,
            role: ROLE_GROUP,
            surface: SURFACE_NONE,
            name: Some(name.clone()),
            content: Some(text.clone()),
            state_text: Some(state.clone()),
            states: STATE_MASK,
            focus: Some(FocusCell { row: 1, col: 2 }),
            selection: Some(SelectionSpan {
                start_col: 0,
                end_col: 1,
            }),
            bounds: Some(Bounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                unit: BOUNDS_UNIT_CHARS,
            }),
        };
        let page = Message::SnapshotPage {
            request: 1,
            index: 0,
            rows: vec![row; MAX_SNAPSHOT_PAGE_ROWS],
            last: true,
        };
        let frame = encode_frame(&page).expect("maximal page encodes");
        assert!(
            frame.len() <= FRAME_HEADER_BYTES + MAX_FRAME_BYTES,
            "frame is {} bytes",
            frame.len()
        );
    }

    #[test]
    fn frame_reader_rejects_truncated_input() {
        let mut reader = FrameReader::new([0_u8, 0, 0, 10, 1, 2][..].as_ref());
        assert_eq!(reader.read_frame(), Err(WireError::Truncated));
        let mut reader = FrameReader::new([0_u8, 0][..].as_ref());
        assert_eq!(reader.read_frame(), Err(WireError::Truncated));
    }

    #[test]
    fn decoder_rejects_oversize_and_invalid_booleans() {
        let oversize = vec![TAG_SHUTDOWN; MAX_FRAME_BYTES + 1];
        assert!(matches!(
            decode_message(&oversize),
            Err(WireError::TooLarge { .. })
        ));

        let mut payload = encode_message(&Message::PublishBegin {
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
        })
        .expect("encode");
        // cursor_visible is the first boolean in this payload; flipping it to
        // 2 must fail closed. Layout: tag, request, generation, root string,
        // title string, cursor row, cursor column, then the boolean.
        let index = 1 + 8 + 8 + 4 + "window".len() + 4 + "shell".len() + 4 + 4;
        assert_eq!(payload[index], 1);
        payload[index] = 2;
        assert_eq!(
            decode_message(&payload),
            Err(WireError::InvalidField("boolean"))
        );
    }

    #[test]
    fn page_counts_match_pages() {
        assert_eq!(page_count(1, MAX_SNAPSHOT_PAGE_ROWS), 1);
        assert_eq!(page_count(16, MAX_SNAPSHOT_PAGE_ROWS), 1);
        assert_eq!(page_count(17, MAX_SNAPSHOT_PAGE_ROWS), 2);
        assert_eq!(diff_page_count(0, 0), 0);
        assert_eq!(diff_page_count(65, 0), 2);
        assert_eq!(diff_page_count(0, 17), 2);
    }
}
