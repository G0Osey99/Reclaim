#!/usr/bin/env bash
# ci.sh — the full local/CI gate (build guide Part 1.4 rule 8):
#   fmt --check · clippy -D warnings · test · cargo deny · check-readonly
#
# `cargo ci` runs this via the scripts/cargo-ci shim (add scripts/ to PATH).
# It is also runnable directly: ./scripts/ci.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }

step "cargo fmt --all --check"
cargo fmt --all -- --check

step "cargo clippy --workspace --all-targets --all-features -- -D warnings"
cargo clippy --workspace --all-targets --all-features -- -D warnings

step "cargo test --workspace --all-features"
cargo test --workspace --all-features

step "cargo deny check"
cargo deny check

step "check-readonly"
scripts/check-readonly.sh

# Recovery-Mode linkage gate (macOS only): the CLI must link only system libs so
# it runs from a Recovery Terminal (docs/plan/06 §4). No-op on Linux.
if [ "$(uname -s)" = "Darwin" ]; then
    step "check-recovery-libs"
    scripts/check-recovery-libs.sh
fi

printf '\n\033[1;32mcargo ci: all gates passed\033[0m\n'
