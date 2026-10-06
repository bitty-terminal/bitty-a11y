# bitty-a11y

Accessibility adapter extension crate (landed CTX-0003 greenfield adapter, independently verified CTX-0004). Zero-dependency snapshot/handle/focus/action core with a headless backend; platform backends follow behind the PlatformBackend trait. Read [AGENTS](AGENTS.md). Task management lives in CarryCtx.

Prerequisite: W-134 / bitty-terminal-docs CTX-0090, Issue #169. Preserve accessible baseline, semantic snapshots, focus and controlled actions; do not silently make accessibility optional.

CTX-0001 -> CTX-0002 -> CTX-0003 -> CTX-0004 maps to Issues #4 -> #3 -> #2 -> #1. All four phases are complete: bootstrap, accepted contract, landed adapter crate with fence tests, and independent verification against the Core W-142 baseline.
