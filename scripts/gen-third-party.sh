#!/usr/bin/env bash
# gen-third-party.sh — regenerate THIRD_PARTY.md from the dependency tree
# (docs/plan/11 §2). Requires cargo-about (`cargo install cargo-about --features cli`).
# Accepted licenses are in about.toml (mirrors deny.toml); the template is about-md.hbs.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"
cargo about generate about-md.hbs -o THIRD_PARTY.md
echo "wrote THIRD_PARTY.md ($(grep -c '^- \[' THIRD_PARTY.md) crates, $(grep -c '^## ' THIRD_PARTY.md) licenses)"
