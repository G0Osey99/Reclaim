#!/usr/bin/env bash
# gen-cli-docs.sh — regenerate docs/cli.md, man pages and shell completions from
# the clap definition (build guide Phase 6 / docs/plan/07). Everything is derived
# from the single source of truth in crates/reclaim-cli, so the docs cannot drift.
#
#   scripts/gen-cli-docs.sh [OUT_DIR]      (default: dist/cli-docs)
#
# Produces OUT_DIR/{cli.md, man/*.1, completions/*} and copies cli.md to docs/.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"

OUT="${1:-dist/cli-docs}"
mkdir -p "$OUT"

# The `docgen` feature compiles the generator into the binary; RECLAIM_GEN_DOCS
# makes it emit and exit before any argument parsing or device access.
RECLAIM_GEN_DOCS="$OUT" cargo run -q -p reclaim-cli --features docgen

cp "$OUT/cli.md" docs/cli.md
echo "wrote docs/cli.md"
echo "  man pages   : $OUT/man/ ($(ls "$OUT/man" | wc -l | tr -d ' ') pages)"
echo "  completions : $OUT/completions/ ($(ls "$OUT/completions" | tr '\n' ' '))"
