# bitty-a11y

Candidate accessibility adapter extension for the Bitty terminal platform. Read [AGENTS](AGENTS.md). Task management lives in CarryCtx.

Status: metadata baseline plus an implemented-only Rust crate. The crate has zero dependencies, forbids unsafe code, and ships a headless backend with fence tests. Product gates cover the adapter, the executable component, the live wire suite, and the packaging shape. The crate is not independently verified. Issues #1, #2, #3, and #4 are closed; the remaining platform and runtime work is tracked in bitty#2257.

Contract: W-134 is accepted in bitty-terminal-docs (`specifications/accessibility-extraction-contract.md`, CTX-0090, Issue #169). The stable host interface is published here (`SnapshotHost` with `Adapter::ingest_host` plus `Adapter::expose_to`); the adapter ingests through `SnapshotBuilder` and defines no private bypass. Core bound this seam in bitty PR #2342 (merged `d2eabc34`) and extended it in bitty PR #2547 (merged `e8fc4d10`): `crates/bitty-ui/src/a11y_host.rs` implements `SnapshotHost` over its terminal projection and the default-off, transport-neutral `ComponentSnapshotHost` for the installable component, W-147 permits the a11y identifiers only in that file, and the parallel `crates/bitty-ui/src/a11y.rs` model is retired. `ComponentHostConfig::default()` enables safe mode and grants no capability, so the component client is a contract seam rather than a default path. A runtime `Adapter` construction/refresh call site and the AT-SPI/D-Bus, Windows UI Automation, and macOS AX backends remain pending (bitty#2257); only `HeadlessBackend` ships, so no assistive technology sees a Bitty tree yet.

## Executable component boundary (bitty#2496)

The crate is also the installable `a11y` native component. It is a stdio
coprocess in the DIR-030 shape: the host spawns `bitty-a11y`, writes
length-prefixed frames on stdin, and reads frames from stdout. stderr carries
diagnostics only. The component wraps the same `SnapshotBuilder` and `Adapter`
path as the linked crate; it carries no installer, download source, capability
policy, safe-mode decision, platform accessibility bridge, or filesystem and
network access.

```text
Core host client  ──stdio frames──▶  bitty-a11y  ──▶  SnapshotBuilder / Adapter
  (remaining work)                    (this repo)       stable ids + bounded deltas
```

- Descriptor: [`bitty-component.toml`](bitty-component.toml) — name `a11y`,
  version `0.0.1`, protocol `[1, 1]`, executable `bitty-a11y`.
- Parser: `bitty_a11y::component::descriptor` — closed `[component]` table,
  strict `X.Y.Z`, `[a-z][a-z0-9-]{0,31}` name, `bitty-<name>` executable, 64
  lowercase hex digits, and a `validate_for_install` check that refuses the
  packaging placeholder digest.
- Server: `bitty_a11y::component::server` — handshake, bounded publication,
  full snapshots, diffs with acknowledgements, stable ids, typed errors,
  request-rate gate, and clean shutdown.
- Binary: `bitty-a11y` (no arguments serves; `--version`;
  `--check-manifest PATH`).

### Wire protocol v1

`frame = length:u32be payload[length]`, `payload = tag:u8 fields...`; integers
are big-endian, strings/list lengths are `u32be` and bounded. The frame cap is
256 KiB and is checked from the four-byte header before any payload read or
allocation. Tags are fixed; unknown tags, trailing bytes, invalid UTF-8, and
invalid booleans fail closed.

| Tag    | Message           | Direction | Fields                                                                                                                                                                                                         |
| ------ | ----------------- | --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `0x01` | `Hello`           | host→comp | `min:u16 max:u16 component:str version:str mode:u8`                                                                                                                                                            |
| `0x02` | `HelloAck`        | comp→host | `protocol:u16 component:str version:str capabilities:u8`                                                                                                                                                       |
| `0x10` | `PublishBegin`    | host→comp | `request:u64 generation:u64 root:str title:str cursor_row:u32 cursor_col:u32 cursor_visible:bool rows:u32 chrome:u32 scene:u32 interactive:u32 notices:u32 modal:bool focus:opt<str> selection:opt<Selection>` |
| `0x11` | `PublishNode`     | host→comp | `request:u64 node:u8 fields...`                                                                                                                                                                                |
| `0x12` | `PublishCommit`   | host→comp | `request:u64 generation:u64`                                                                                                                                                                                   |
| `0x13` | `SnapshotRequest` | host→comp | `request:u64 since:u64`                                                                                                                                                                                        |
| `0x14` | `DiffAck`         | host→comp | `generation:u64`                                                                                                                                                                                               |
| `0x3F` | `Shutdown`        | host→comp | none                                                                                                                                                                                                           |
| `0x20` | `Published`       | comp→host | `request:u64 generation:u64 nodes:u32 added:u32 updated:u32 removed:u32`                                                                                                                                       |
| `0x21` | `SnapshotBegin`   | comp→host | `request:u64 generation:u64 nodes:u32 focus:opt<u64> selection:opt<Selection> pages:u32`                                                                                                                       |
| `0x22` | `SnapshotPage`    | comp→host | `request:u64 index:u32 rows:list<TreeRow> last:bool`                                                                                                                                                           |
| `0x23` | `DiffBegin`       | comp→host | `request:u64 base:u64 generation:u64 removed:u32 upserts:u32 focus:opt<u64> selection:opt<Selection> pages:u32`                                                                                                |
| `0x24` | `DiffPage`        | comp→host | `request:u64 index:u32 removed:list<u64> upserts:list<TreeRow> last:bool`                                                                                                                                      |
| `0x2F` | `Error`           | comp→host | `request:u64 kind:u8 message:str`                                                                                                                                                                              |
| `0x3E` | `ShutdownAck`     | comp→host | none                                                                                                                                                                                                           |

`PublishNode` carries exactly one projection entry: terminal row
(`row:u32 text:str`), chrome (`kind:u8 name:str state:opt<str>`), scene
(`kind:u8 identity:u64`), interactive (`purpose:u8 name:str enabled:bool`), or
notice (`kind:u8 text:str`). `PublishBegin` declares exact entry counts; a
stream that disagrees is refused before anything swaps.

Tree rows carry `id:u64 parent:u64 depth:u8 role:u8 surface:u8`, optional name,
terminal content, chrome state text, state bits, cursor `focus`, per-row
`selection` span, and logical `bounds` (characters, never pixels). Roles are
the fixed v1 vocabulary or `role = none` for a chrome surface identified by
`surface`.

Properties the implementation enforces:

- **Projection only.** The wire names projection content, generations,
  request ids, and bounded tree rows. It carries no path, argv, environment,
  file descriptor, URL, socket, or platform command. Error messages are
  bounded static diagnostics and never echo names or text.
- **Stable ids.** An id is assigned to the projection anchor on first sight
  and survives while the anchor survives. A retired id is never reused; a
  re-added anchor gets a new id.
- **Defined stale-tree behavior.** A request for a diff from an older or
  unknown generation returns a full snapshot, never a fabricated delta. A
  publication at or below the live generation fails with
  `stale_generation`; an ack for a generation the server has not published
  fails the same way.
- **Bounded everything.** 4096 nodes, depth 4, 16 rows per page, 64 removed
  ids per diff page, 8 KiB per terminal run, a 1 MiB publication text budget,
  a 120-request burst at 60 requests/second, and a 256 KiB frame cap. A
  sustained flood closes the connection.
- **Safe mode and deprivation.** `Hello.mode` bit `0x01` is safe mode and bit
  `0x02` is the host's exposure grant. Safe mode, a withheld grant, or both
  completes the handshake with zero capabilities and refuses publication,
  snapshot, and ack exchanges with `denied`; shutdown still works. The
  component never self-grants.
- **Clean shutdown and crash containment.** `Shutdown` or stdin EOF exits 0;
  a framing or decode violation writes a connection-level `Error` and exits 3. The server starts no threads. A crashed component cannot kill the host,
  and the host can restart it; Core must treat EOF as `component_lost`.

### Packaging

```bash
just component-package        # build Linux x86_64 and pack dist/x86_64-unknown-linux-gnu.tar.gz
just component-package-test   # package-shape test without cargo, network, or R2
just component-smoke          # live protocol suite against the debug binary
```

`scripts/package-component.sh` builds (or accepts `--binary`) one Rust target
and writes exactly `<target>.tar.gz`, `<target>.tar.gz.sha256`, and
`SHA256SUMS`. The archive contains exactly `bitty-component.toml` (with the
target binary's real digest) and `bitty-a11y` (`bitty-a11y.exe` on Windows).
It validates the canonical record, the Cargo version, the target name, and
every existing archive's version before writing, so a mixed-version directory
cannot be published. Only Linux x86_64 is built and tested in this task.
The #2491 per-platform matrix and catalog rows remain follow-ups. No tag is
created and nothing is uploaded to R2.

### Verification

```bash
just product                                   # fmt, clippy -D warnings, all tests
just msrv                                      # MSRV floor: cargo +1.85.0 check
cargo test --test component_server --locked    # live protocol suite
cargo test --test component_descriptor --locked
```

The CI `MSRV 1.85 check` job installs Rust 1.85.0 and compiles
`--workspace --all-targets` with `-D warnings`, so syntax newer than the
declared `rust-version = "1.85"` cannot land again.

The live suite covers a real publication and snapshot, diff/ack, stable ids
that are never reused, stale-generation refusal, oversized/malformed/zero
length frames, the publication text budget, safe-mode and capability
deprivation, rate exhaustion, and a SIGKILLed component whose host survives
and restarts it. Set `BITTY_A11Y_COMPONENT_BIN` to run the same suite against a
packed release binary.

### Remaining for the Core host client

- Resolve/install the descriptor, verify name/version/protocol/executable and
  the streamed SHA-256, and spawn the binary from its version directory.
- Serialize the projection off the UI thread and publish it in bounded
  batches; own the handshake deadline, request ids, per-request deadlines,
  capability grants, consent, and safe-mode denial.
- Treat `Error`/EOF/exit-3 as `component_lost`, fail in-flight exchanges,
  restart with the existing backoff policy, and place the component in its own
  process group/session so a component crash can be cleaned up by killing the
  group.
- Implement the AT-SPI/D-Bus (Linux), UI Automation (Windows), and macOS AX
  backends behind the served rows; this repository ships the protocol and the
  headless projection only.
- Run parity tests against the linked projection and remove the direct Core
  `bitty-ui` dependency after parity. Add the remaining target matrix (#2491)
  and synchronize the canonical docs.

## Layout

`Cargo.toml` (publish false), `src/` (adapter core plus `component/` and
`src/bin/bitty-a11y.rs`), `tests/` (fences, headless round-trip, host
conformance, component descriptor, live component server), `scripts/`
(packaging and CarryCtx publication), `docs/` (design notes), `justfile`
(metadata, component, and product gates), CI (metadata, Rust, MSRV 1.85,
actionlint, component package and live smoke), and publication ref
`refs/heads/carryctx-snapshots`.

Phases: CTX-0001 maps to #4, CTX-0002 to #3, CTX-0003 to #2, CTX-0004 to #1.
Closeout tasks CTX-0005, CTX-0006, and CTX-0007 track the open PRs; CTX-0009
adds the component-server half of bitty#2496.
