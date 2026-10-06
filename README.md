# bitty-a11y

Candidate accessibility adapter extension for the Bitty terminal platform. Read [AGENTS](AGENTS.md). Task management lives in CarryCtx.

Status: metadata baseline plus an implemented-only Rust crate. The crate has zero dependencies, forbids unsafe code, and ships a headless backend with fence tests. Local product gates pass (21 tests: 18 fences, 3 headless round-trip). The crate is not independently verified. GitHub Issues #4, #3, and #2 remain open pending review; Issue #1 is closed.

Contract: W-134 is accepted in bitty-terminal-docs (`specifications/accessibility-extraction-contract.md`, CTX-0090, Issue #169). Core integration (W-142) is pending a stable host interface; the adapter ingests through `SnapshotBuilder` and defines no private bypass.

Layout: `Cargo.toml` (publish false), `src/` (adapter core), `tests/` (fences, headless round-trip), `justfile` (metadata gates, product gates), CI (metadata, Rust, actionlint), and publication ref `refs/heads/carryctx-snapshots`.

Phases: CTX-0001 maps to #4, CTX-0002 to #3, CTX-0003 to #2, CTX-0004 to #1. Closeout tasks CTX-0005, CTX-0006, and CTX-0007 track the open PRs.
