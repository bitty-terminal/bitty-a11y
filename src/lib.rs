//! `bitty-a11y`: standalone platform-accessibility adapter core.
//!
//! Removal-first, thin host, one-way dependencies: this extension
//! crate depends on Core, never the reverse, and today it depends on
//! nothing at all — not even Core. The Core baseline types
//! (`bitty-ui::a11y`) are explicitly candidate, implemented-only
//! evidence, not a stable export, so a pinned dependency would freeze
//! the adapter to unstable spellings and drag the Core monorepo along
//! with it. Instead this crate defines the adapter-side consumer shape
//! directly against the accepted contract below and carries zero
//! dependencies (standard library only).
//!
//! # Contract
//!
//! The accepted terminal-side contract is
//! `bitty-terminal-docs/specifications/accessibility-extraction-contract.md`
//! (`W-134`, Accepted): a bounded, immutable, read-only semantic
//! snapshot of one accessibility root; generation-paired unforgeable
//! handles; focus association owned by Core with fail-closed mismatch;
//! a controlled action interface over the closed v1 set (activation of
//! a declared interactive node); coalesced announcements with no
//! timer; no Event-Bus publication; nothing persisted.
//!
//! The projection is never authority: building or updating a snapshot
//! cannot mutate grid, cursor, modes, scrollback, attachment, focus,
//! or policy, and removing the adapter removes platform exposure only.
//!
//! # Decisions fixed here (contract open points owned by this task)
//!
//! - Accepted numeric ceilings: names and labels 256 characters,
//!   snapshots 4096 nodes, announcement text 256 characters, pending
//!   announcements 8 (see [`MAX_ACCESSIBLE_NAME_LEN`],
//!   [`MAX_SNAPSHOT_NODES`], [`MAX_ANNOUNCEMENT_TEXT_LEN`],
//!   [`MAX_PENDING_ANNOUNCEMENTS`]).
//! - Role vocabulary: the fixed v1 spelling on [`Role`] (backends map
//!   it to the native platform model).
//! - Terminal text: text runs only, never styling attributes (the
//!   fidelity boundary).
//! - Announcement coalescing: consecutive-duplicate collapse plus
//!   drop-oldest overflow, with no time window — the no-periodic-timer
//!   rule forbids one.
//!
//! # Open points (honest, owned downstream)
//!
//! - Core snapshot export (`W-142` / `CTX-0935`): Core does not yet
//!   publish its projection generation, focus owner, or invalidation
//!   source through a stable public interface. Until it does, the host
//!   feeds this adapter through [`SnapshotBuilder`] and
//!   [`Adapter::ingest`]; no Core code is vendored here and no private
//!   bypass exists.
//! - Per-platform backends (AT-SPI/D-Bus, Windows UI Automation,
//!   macOS AX) materialize [`PlatformBackend`] in follow-up tasks;
//!   only the [`HeadlessBackend`] ships here.
//! - Action taxonomy beyond node activation and any live event stream
//!   require their own reviewed extension (post-1.0).
//! - Contrast floor ownership stays with the theme-token contract;
//!   screen-reader traversal mode is parked.

#![forbid(unsafe_code)]

pub mod action;
pub mod adapter;
pub mod announce;
pub mod backend;
pub mod error;
pub mod focus;
pub mod handles;
pub mod snapshot;

pub use action::{ActionKind, ActionOutcome, ActionRequest, ActionSink};
pub use adapter::{Adapter, InvalidationSource, needs_refresh};
pub use announce::{
    Announcement, AnnouncementKind, AnnouncementQueue, MAX_ANNOUNCEMENT_TEXT_LEN,
    MAX_PENDING_ANNOUNCEMENTS,
};
pub use backend::{ExposedSnapshot, HeadlessBackend, PlatformBackend};
pub use error::A11yError;
pub use focus::{FocusAssociation, FocusOwner, resolve_focus};
pub use handles::{Anchor, ElementHandle, Generation, HandleMap};
pub use snapshot::{
    ChromeKind, CursorPosition, FocusScope, InteractivePurpose, MAX_ACCESSIBLE_NAME_LEN,
    MAX_SNAPSHOT_NODES, NodeView, Role, SceneKind, Snapshot, SnapshotBuilder, TextRun, role_of,
};
