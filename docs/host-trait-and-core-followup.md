# Host trait and Core follow-up: retire `bitty-ui/src/a11y.rs`

Priority: P1 | Area: architecture | Labels: chore,P1,area:architecture
| Milestone: v0.1.0 | Task: CTX-0008 | Issue: a11y#14 | Audit: bitty#1629 F4

This note publishes the host-side contract and records the Core-side
follow-up as a doc-level reference only. No Core changes land here.
Core rewire happens after 0.0.23.

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

## Core follow-up (doc reference only)

Core still carries the parallel model named in a11y#14:
`crates/bitty-ui/src/a11y.rs` duplicates this repo's `snapshot.rs`
(`Role`, `SceneKind`, `ChromeKind`, `SnapshotBuilder`, same constants
`MAX_A11Y_TREE_NODES = 4096`, `MAX_ACCESSIBLE_NAME_LEN = 256`), with
the single consumer `uitree.rs`. This repo additionally owns the
`Adapter`, `ingest`, and `expose_to` seam Core lacks.

The Core rewire, tracked from bitty#1629 after the 0.0.23 release,
implements `SnapshotHost` on the Core side, drives
`Adapter::ingest_host` plus `Adapter::expose_to`, migrates the single
`uitree.rs` consumer, and deletes `crates/bitty-ui/src/a11y.rs`. That
work lands in the Core repository, not here.
