# Metadata-only gates. These are not Rust/Lua product evidence.
prettier_version := "3.9.6"
markdownlint_version := "0.23.1"
actionlint_version := "1.7.12"

fmt-check:
    bunx --bun prettier@{{prettier_version}} --check . --ignore-unknown

markdownlint:
    bunx --bun markdownlint-cli2@{{markdownlint_version}}

metadata:
    test -s README.md && test -s AGENTS.md && test -s repo.toml
    test -s .carryctx/config.toml
    python3 -c 'import tomllib; from pathlib import Path; [tomllib.loads(p.read_text()) for p in [Path("repo.toml"), Path(".carryctx/config.toml")]]'

component-metadata:
    #!/usr/bin/env bash
    set -euo pipefail
    test -s bitty-component.toml
    version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
    test -n "$version"
    grep -Fx '[component]' bitty-component.toml >/dev/null
    grep -Fx 'name = "a11y"' bitty-component.toml >/dev/null
    grep -Fx "version = \"$version\"" bitty-component.toml >/dev/null
    grep -Fx 'protocol = [1, 1]' bitty-component.toml >/dev/null
    grep -Fx 'executable = "bitty-a11y"' bitty-component.toml >/dev/null
    grep -Fx 'sha256 = "0000000000000000000000000000000000000000000000000000000000000000"' bitty-component.toml >/dev/null
    test -x scripts/package-component.sh
    test -x scripts/test-package-component.sh

hygiene:
    #!/usr/bin/env bash
    set -euo pipefail
    bad=0
    while IFS= read -r -d '' f; do
        case "$f" in
            *.sqlite|*.db|*.zst|*.tar|*.tar.gz|*.tgz|node_modules/*|target/*|dist/*|.worktrees/*)
                echo "unexpected artifact tracked: $f" >&2; bad=1 ;;
        esac
    done < <(git ls-files -z --cached --others --exclude-standard)
    test "$bad" -eq 0

paths:
    #!/usr/bin/env bash
    set -euo pipefail
    pattern='(/hom''e/|/Use''rs/|/mn''t/[A-Za-z]|[A-Za-z]:[\\/]Use''rs[\\/])'
    found=0
    while IFS= read -r -d '' f; do
        if grep -nEI "$pattern" "$f"; then found=1; fi
    done < <(git ls-files -z --cached --others --exclude-standard)
    if [ "$found" -ne 0 ]; then
        echo 'hardcoded host path detected (portable-path gate)' >&2
        exit 1
    fi
    echo 'portable-path gate passed'

actionlint:
    @installed="$(actionlint --version | head -n 1)"; test "$installed" = "{{actionlint_version}}" || { echo "actionlint {{actionlint_version}} required; found $installed" >&2; exit 1; }
    actionlint -color -shellcheck=

check:
    just fmt-check
    just markdownlint
    just metadata
    just component-metadata
    just hygiene
    just paths

# Build the Linux x86_64 executable component and pack its release archive.
component-package:
    ./scripts/package-component.sh --target x86_64-unknown-linux-gnu --output dist/x86_64-unknown-linux-gnu.tar.gz

# Shape test for the packager (no cargo build, network, or R2).
component-package-test:
    ./scripts/test-package-component.sh

# Drive the live protocol suite against the just-built debug binary.
component-smoke:
    cargo test --test component_server --locked

workflows:
    actionlint
    act -n

# Publish a redacted CarryCtx snapshot to refs/heads/carryctx-snapshots.
workflow-publish:
    ./scripts/workflow-publish.sh

# Fetch and import the published CarryCtx snapshot (fresh-clone recovery).
workflow-import:
    git fetch origin refs/heads/carryctx-snapshots:refs/remotes/origin/carryctx-snapshots
    carryctx import --from-git refs/remotes/origin/carryctx-snapshots

# Product gates: full Rust verification for the landed crate.
product:
    cargo fmt --all -- --check
    RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets
    cargo test --workspace --locked

# MSRV floor gate: the declared rust-version in Cargo.toml (CI installs it).
msrv:
    RUSTFLAGS="-D warnings" cargo +1.85.0 check --workspace --all-targets --locked
