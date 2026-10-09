# Phase 3 implementation evidence: standalone adapter without bypass

Priority: P1 | Area: architecture | Labels: chore,P1,area:architecture
| Milestone: v0.1.0 | RFC: W-134 | Task: CTX-0003

Phase 3 implements the accepted standalone component without a private
host bypass. Scope is this repository only. This note assembles the
acceptance evidence for the landed crate. It adds no product code.

## Landed implementation

Commits on `main`:

- `8e17c04` builds the accessibility adapter crate (W-142): 9 modules
  (`action`, `adapter`, `announce`, `backend`, `error`, `focus`,
  `handles`, `lib`, `snapshot`), 2 fence suites plus headless
  round-trip.
- `eae7970` adds the Rust quality-gates CI job (W-142).

Crate shape:

- `Cargo.toml`: zero dependencies (standard library only),
  `publish = false`, edition 2024, `rust-version = 1.85`,
  `forbid(unsafe_code)`, clippy pedantic warnings.
- `src` totals 9 modules; `tests` holds `fences.rs` (18 tests) and
  `headless_roundtrip.rs` (3 tests), 21 tests total.
- Product gates pass: `cargo fmt --check`, `clippy` with `-D warnings`,
  `cargo test` 21 passed.

## No private host bypass

- No Core dependency: `Cargo.toml` carries no dependency section. No
  Core code is vendored here.
- Ingest is through the public adapter shape: a host can feed this
  adapter through `SnapshotBuilder` and `Adapter::ingest`. Building or
  updating a snapshot cannot mutate grid, cursor, modes, scrollback,
  attachment, focus, or policy.
- Only `HeadlessBackend` ships. Per-platform backends (AT-SPI/D-Bus,
  Windows UI Automation, macOS AX) materialize `PlatformBackend` in
  follow-up tasks and are deferred per the crate open points.
- Fixed decisions owned here: names and labels 256 characters,
  snapshots 4096 nodes, announcement text 256 characters, pending
  announcements 8; fixed v1 `Role` vocabulary; text runs only (no
  styling); announcement coalescing with duplicate collapse plus
  drop-oldest overflow and no timer.

## Contract prerequisites

- W-134 accepted in `bitty-terminal-docs`
  (`specifications/accessibility-extraction-contract.md`, CTX-0090,
  Issue 169): bounded immutable read-only snapshot, generation-paired
  handles, Core-owned focus association, controlled v1 action set,
  coalesced announcements, no Event-Bus publication, nothing persisted.
- Core W-142 (`bitty#1626`, CTX-0935) closed as verified no-op: zero
  adapter-shaped code in Core; the Core baseline mechanism is retained
  and the adapter lives here. No Core deletion was required. The Core
  snapshot export through a stable public interface stays a downstream
  open point; until it lands the host uses `SnapshotBuilder` with no
  private bypass.

## Narrow file scope

This change adds only this evidence note. No product code changes. No
Core edits. Prior closeout history (`#7` README, `#8` boundary) is
preserved; this note cites the landed implementation as
implemented-only evidence pending independent verification in Phase 4.

## Verification

- `just check` green (prettier, markdownlint, metadata, hygiene,
  portable-path gate).
- Product gates green: `cargo fmt --check`, `clippy -D warnings`,
  `cargo test` 21 passed (18 fences plus 3 headless round-trip).
- English-only, no invented identifiers, no host paths.
- Independent review required. This PR does not merge itself and does
  not approve its own work. Phase 4 verification (different reviewer,
  platform evidence, docs sync) belongs to the owning task.

## Open points

- Core snapshot export through a stable public interface (W-142
  follow-up).
- Per-platform backends, action taxonomy beyond node activation, live
  event stream, contrast floor ownership, and traversal mode per the
  crate open points.
