# Accessibility component server (bitty#2496)

Status: implemented-only, pending independent review. This note describes the
server half of the `bitty-a11y` executable-component split. The Core host
client, the `SnapshotHost` migration, and removal of Core's direct `bitty-ui`
dependency are follow-ups and are not implemented here.

## Purpose and scope

The repository already ships a dependency-free adapter with the accepted
`SnapshotHost` seam. This component adds the installable form required by the
component split:

- `bitty-component.toml` records the fixed component identity: name `a11y`,
  version `0.0.1`, protocol `[1, 1]`, executable `bitty-a11y`.
- `src/component/descriptor.rs` parses that record with a closed, fail-closed
  reader and refuses the packaging placeholder digest at install validation.
- `src/component/protocol.rs` is the authoritative v1 codec for bounded
  accessibility snapshots and diffs.
- `src/component/server.rs` owns the live projection, stable element ids, and
  bounded responses.
- `src/bin/bitty-a11y.rs` serves the protocol on stdin/stdout or validates a
  packaged descriptor with `--check-manifest`.
- `scripts/package-component.sh` builds one target and packs the DIR-030
  archive plus checksums. `scripts/test-package-component.sh` checks its shape
  without a Rust build.

Out of scope: install/download, capability grants, consent, safe-mode policy,
platform accessibility bridges, target matrices, and Core changes.

## Authority boundary

Core owns terminal truth, the projection decision, focus, selection, the
host grant, and safe-mode denial. The component is mechanism only:

- It accepts projection content only through an explicit publication exchange.
- It validates the content with the same `SnapshotBuilder` and `Adapter` path
  as the linked crate, so a second accessibility model is not defined here.
- It carries no path, argv, environment, file descriptor, URL, socket, or
  platform command in the wire schema.
- It never persists content and never publishes it on an event bus.

## Wire protocol v1

Framing is `length:u32be` followed by a payload of `tag:u8` and fields.
Integers are big-endian. Strings are `len:u32be` plus UTF-8 and every length
is bounded. Lists are `count:u32be` plus bounded elements. Booleans are `0x00`
or `0x01` only. Optional values are `0x00` or `0x01` plus the value.

### Full snapshot path

1. The host sends `Hello` with its version range and a mode byte.
2. The component replies `HelloAck` with protocol `1`, its name and version,
   and capability bits (zero when deprived).
3. The host sends `PublishBegin`, one `PublishNode` per projection entry, then
   `PublishCommit`. The commit declares the generation; it must be greater
   than the live generation.
4. The component validates counts, names, structure, roles, and the node cap,
   assigns stable ids per projection anchor, swaps the projection atomically,
   and replies `Published`.
5. The host asks with `SnapshotRequest` (`since = 0` for a full tree). The
   component replies with `SnapshotBegin` and one or more `SnapshotPage`
   frames. The last page is flagged.
6. After applying a delta the host sends `DiffAck` with the live generation.
   That advances the acknowledged baseline.

### Diff path

`SnapshotRequest` with `since` equal to the acknowledged baseline returns
`DiffBegin` and zero or more `DiffPage` frames. `removed` lists retired ids;
`upserts` lists added or changed rows in tree order. `pages = 0` is a valid
empty diff.

If `since` is the live generation, the component returns an empty diff. If
`since` is older than the acknowledged baseline or unknown, it returns a full
snapshot. A `since` value ahead of the live generation is a typed
`stale_generation` error.

### Tree rows

Each row carries:

- `id` — stable non-zero element id;
- `parent` and `depth` — flat preorder tree shape;
- `role` — fixed v1 role code, or `role = none` for a chrome surface;
- `surface` — chrome surface code (bar, rail, tablist, notification, overlay);
- `name` — bounded accessible name when the node has one;
- `content` — terminal row text (never a name);
- `state_text` — bounded chrome state, when any;
- `states` — focusable, focused, enabled, selected, modal, cursor-visible;
- `focus` — the terminal cursor cell on the terminal leaf;
- `selection` — the selected character span on a terminal row;
- `bounds` — logical character bounds on terminal rows.

Ids are derived from projection anchors: root, terminal leaf, terminal row
ordinal, chrome surface kind, producer scene identity, and interactive purpose
plus name. They are never positional indices. A retired id is not reused;
re-adding an anchor produces a new id. A host that holds an id across a
generation boundary must re-request a snapshot when it does not recognize the
new tree.

## Bounds

| Bound                       | Value                       | Failure behavior                    |
| --------------------------- | --------------------------- | ----------------------------------- |
| Frame payload               | 256 KiB                     | connection-level `Error`, exit 3    |
| Snapshot nodes              | 4096                        | `invalid_request` or `budget`       |
| Tree depth                  | 4                           | `invalid_request`                   |
| Snapshot page rows          | 16                          | decoder refuses                     |
| Diff page upserts           | 16                          | decoder refuses                     |
| Diff page removed ids       | 64                          | decoder refuses                     |
| Terminal run text           | 8 KiB                       | decoder refuses                     |
| Accessible name / state     | 256 characters, 1 KiB UTF-8 | `invalid_request`                   |
| Publication text            | 1 MiB total                 | `budget`, staging cleared           |
| Publication entries         | 4104                        | `invalid_request`                   |
| Request rate                | 120 burst, 60 per second    | `rate_limited`, 16 violations close |
| Selection/cursor coordinate | 1,000,000 rows, 65,536 cols | decoder refuses                     |

The server starts no threads. A host that stops reading applies ordinary pipe
backpressure instead of growing an unbounded queue.

## Safe mode and deprivation

The host declares policy in `Hello.mode`:

- `0x01` — safe mode;
- `0x02` — the host granted the accessibility exposure capability.

Safe mode, a missing grant, or both make the session deprived. The handshake
still completes, with `capabilities = 0`. Publication, snapshot, and ack
exchanges return `Error { kind: denied }` without staging or swapping
anything. `Shutdown` remains available. The component never self-grants and
never infers a grant from the platform.

## Stale-tree behavior

- A publication generation at or below the live generation is refused with
  `stale_generation`; nothing swaps.
- An ack for a generation other than the live generation is refused with
  `stale_generation`; the baseline does not move.
- A snapshot request ahead of the live generation is refused with
  `stale_generation`.
- A snapshot request from an older or unknown generation returns a full
  snapshot. The component never fabricates a delta from a tree it no longer
  has.
- A removed id stays removed. Re-adding the same anchor creates a fresh id, so
  a host cannot accidentally act on a retired element.

## Crash and shutdown

- `Shutdown` writes `ShutdownAck` and exits 0.
- stdin EOF exits 0 without a frame.
- A framing, decode, or handshake violation writes a connection-level `Error`
  and exits 3.
- An internal write failure exits 4.
- A component crash cannot kill the host. The host must treat EOF or a
  nonzero exit as `component_lost`, fail in-flight exchanges, and run the
  component in its own process group so the whole group can be reaped.

## Packaging

`bitty-component.toml` is the canonical record with a zero-digest sentinel.
`scripts/package-component.sh` refuses any drift in the fixed fields, renders
the real executable digest into a staged descriptor, and writes:

- `dist/<target>.tar.gz` containing exactly `bitty-component.toml` and
  `bitty-a11y` (`bitty-a11y.exe` on Windows);
- `dist/<target>.tar.gz.sha256`;
- `dist/SHA256SUMS` covering every target archive in the directory.

The script re-verifies every existing archive's descriptor version before
adding a target, so a mixed-version directory cannot be published. No tag,
catalog row, or R2 upload belongs to this work. Only Linux x86_64 is built and
verified; the per-platform matrix is bitty#2491.

## Verification plan

- `cargo test --lib` covers descriptor parsing, codec round trips, malformed
  payload rejection, server staging, stale generation, selection clipping,
  and safe-mode refusal.
- `tests/component_descriptor.rs` pins the repository record, protocol
  constants, exit codes, `--version`, and `--check-manifest`.
- `tests/component_server.rs` spawns the real binary and covers publication
  and snapshot, diff/ack, id stability, stale ids, stale generations,
  oversized/malformed/zero-length frames, the publication budget, safe-mode
  and capability deprivation, rate exhaustion, and crash restart.
- CI builds the Linux x86_64 release binary, packs the archive, verifies
  checksums, and runs the live suite against the packed executable.

Set `BITTY_A11Y_COMPONENT_BIN` to run the live suite against any packed
release binary.

## Limitations and open points

- No AT-SPI/D-Bus, UI Automation, or macOS AX bridge ships here. The served
  rows are the protocol contract; platform materialization is a follow-up.
- No Core host client, installer, digest verification, deadline, crash
  accounting, or linked-dependency removal ships here.
- Publication is a whole projection per generation, delivered in bounded
  entry frames. A future extension may add incremental publication; v1 keeps
  the model simple and bounded.
- Announcements are accepted for parity with the linked adapter and bounded
  by the existing queue, but no announcement stream is exposed on the wire
  in v1. Chrome `state_text` carries announced state.
- Only Linux x86_64 is built and tested; other targets remain unverified until
  bitty#2491.
