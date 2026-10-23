//! Executable-component boundary for the accessibility adapter.
//!
//! The crate still exposes the in-process [`Adapter`](crate::Adapter) and the
//! [`SnapshotHost`](crate::SnapshotHost) seam for Core's linked use; this
//! module adds the installable form required by the component split
//! (bitty#2496): a validated `bitty-component.toml`, a bounded v1 stdio wire
//! protocol for accessibility snapshots and diffs, and a server binary that
//! owns the projection, stable element ids, and bounded deltas.
//!
//! It carries no installer, no download source, no capability policy, no
//! platform accessibility bridge, and no packaging logic beyond the
//! repository script. Core remains the projection producer, the capability
//! authority, and the safe-mode decision owner; this component is mechanism
//! only.
//!
//! ```text
//! Core host client  ──stdio frames──▶  bitty-a11y  ──▶  SnapshotBuilder
//!   (remaining work)                    (this module)      Adapter
//!                                          │
//!                                          └──▶ bounded Snapshot / Diff rows
//! ```
//!
//! # Boundaries fixed here
//!
//! - The wire is the only input. [`Publish`](protocol::Message::PublishBegin)
//!   carries the same projection fields Core already feeds through
//!   [`SnapshotHost`](crate::SnapshotHost); it carries no path, argv,
//!   environment, file descriptor, URL, or platform command.
//! - The server validates every publication with the same
//!   [`SnapshotBuilder`](crate::SnapshotBuilder) and
//!   [`Adapter`](crate::Adapter) path as the linked crate, then answers with
//!   canonical tree rows that carry a stable element id (the projection
//!   anchor), role, name, states, focus/selection, and bounds.
//! - Every response is chunked so one frame stays under
//!   [`MAX_FRAME_BYTES`](protocol::MAX_FRAME_BYTES); snapshots, diffs,
//!   publications, page sizes, node counts, tree depth, text bytes, and
//!   request rate are capped.
//! - Safe mode and a withheld capability are host declarations; the component
//!   refuses every projection exchange with a typed `denied` error while
//!   still completing the handshake and shutdown. It never self-grants.

pub mod descriptor;
pub mod protocol;
pub mod server;

/// Component version reported by `HelloAck`, `--version`, and the descriptor.
pub const COMPONENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use descriptor::{
    COMPONENT_EXECUTABLE, COMPONENT_NAME, COMPONENT_PROTOCOL_MAX, COMPONENT_PROTOCOL_MIN,
    ComponentDescriptor, DescriptorError, MAX_DESCRIPTOR_BYTES, PLACEHOLDER_SHA256,
};
pub use protocol::{
    CAPABILITY_ANNOUNCEMENTS, CAPABILITY_DIFF, CAPABILITY_SELECTION, CAPABILITY_SNAPSHOT,
    ErrorKind, FrameReader, HELLO_CAPABILITY_GRANTED, HELLO_MODE_MASK, HELLO_SAFE_MODE,
    MAX_DIFF_PAGE_REMOVED, MAX_DIFF_PAGE_ROWS, MAX_FRAME_BYTES, MAX_GRID_COLS, MAX_GRID_ROWS,
    MAX_PUBLISH_ENTRIES, MAX_PUBLISH_TEXT_BYTES, MAX_ROW_TEXT_BYTES, MAX_SNAPSHOT_PAGE_ROWS,
    MAX_TREE_DEPTH, Message, PROTOCOL_VERSION, PublishNode, Selection, TreeRow, WireError,
    decode_message, encode_frame, encode_message, negotiate, write_frame,
};
pub use server::{
    SERVER_EXIT_INTERNAL, SERVER_EXIT_OK, SERVER_EXIT_PROTOCOL, SERVER_EXIT_REFUSED, Session,
    SessionOutcome, serve, serve_stdio,
};
