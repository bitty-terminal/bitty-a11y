# Host trait and Core binding status

Priority: P1 | Area: architecture | Labels: chore,P1,area:architecture
| Milestone: v0.1.0 | Task: CTX-1362 | Issue: a11y#14 | Refs: bitty#2257

This note publishes the host-side contract and records the Core-side
binding status. No Core changes land here.

## Host contract

The stable host interface is `SnapshotHost` (`src/host.rs`) with
`Adapter::ingest_host` plus `Adapter::expose_to`. Normative docs live
in `src/host.rs`; this note summarizes.

- Ingest: the caller builds a `SnapshotBuilder` for a `Generation`,
  the host fills it through `SnapshotHost::build_snapshot`, the build
  finishes to a `Snapshot` plus a `HandleMap`, focus binds through
  `SnapshotHost::focus_for`, notices come from
  `SnapshotHost::announcements`, and `Adapter::ingest` swaps
  atomically. `Adapter::ingest_host` drives the full path so Core and
  tests share one seam.
- Expose: `Adapter::expose_to` materializes the live snapshot in a
  `PlatformBackend` when permission is granted;
  `Adapter::announce_to` drains queued notices in push order. A
  generation advance retires the previous generation through
  `PlatformBackend::retire`.
- Bounded inputs: names and labels 256 characters
  (`MAX_ACCESSIBLE_NAME_LEN`), snapshots 4096 nodes
  (`MAX_SNAPSHOT_NODES`), announcement text 256 characters
  (`MAX_ANNOUNCEMENT_TEXT_LEN`), pending announcements 8
  (`MAX_PENDING_ANNOUNCEMENTS`). Violations fail closed; silent
  truncation is non-conforming.
- Error mapping: host failures surface as `A11yError` only. Unmapped
  `SceneKind` or purpose maps to `UnmappedSceneKind`, overlong names
  to `NameTooLong`, oversize builds to `TooManyNodes`, missing root or
  terminal data or a modal overlay without an overlay surface to
  `InvalidStructure`, superseded generations to `StaleGeneration`,
  focus disagreement to `FocusMismatch`, and refused exposure to
  `PermissionDenied`.
- No retained handles: the host must not store `Snapshot`,
  `HandleMap`, `ElementHandle`, or `FocusOwner` beyond the ingest
  call. Only persistent identity persists across ingests: row
  ordinals, producer-assigned scene identities, kind and purpose
  spellings, accessible names, and the `Generation` itself.

## Conformance

`tests/host_conformance.rs` pins ingest and expose behavior against a
fake host that holds owned values only and re-binds focus fresh on
every ingest. It covers the round trip through `HeadlessBackend`,
all fail-closed bounds, stale-generation rejection with nothing
swapped, retirement on generation advance, and no retained handles
across generations.

## Core binding status

Core bound this seam in bitty PR #2547 (merged `e8fc4d10`;
CTX-1466, Refs bitty#2257). `crates/bitty-ui/src/a11y_host.rs`
implements `SnapshotHost` over the terminal projection,
`crates/bitty-ui/src/a11y.rs` is retired, and `uitree.rs` consumes
this repo's `Role` vocabulary. The W-147 fence permits the
`bitty-a11y` identifiers only in `a11y_host.rs`, so the linked model
and the component protocol stay a source-restricted seam owned by
`bitty-ui`.

The same file implements `ComponentSnapshotHost`, the client half of
the merged executable component (`f7e57091`, follow-up `99486eef`).
It is default-off and transport-neutral: `ComponentHostConfig::default()`
enables safe mode and grants no capability, `connect` denies before
any exchange, and the client validates the handshake, complete
snapshots, and diffs before publication. It performs no blocking I/O
on the UI thread and exposes no actions, process spawning,
filesystem/network operations, or platform accessibility API. The
linked `SnapshotHost` model remains the safe fallback, and the
component alias must not become the default path without
compatibility and security review.

The following remain pending: a runtime `Adapter` construction and
refresh call site that drives `ingest_host` and `expose_to`; the
composition-root transport, installer, digest policy, and
process-group lifecycle for the component; and the platform backends
(AT-SPI/D-Bus on Linux, Windows UI Automation, macOS AX). Only
`HeadlessBackend` ships. No assistive technology, including a screen
reader, sees a Bitty tree yet: the merged seam is a contract, not
platform exposure. The remaining platform and runtime work is tracked
in bitty#2257.
