#!/usr/bin/env bash
# build-recovery.sh — build the universal Recovery-Mode `reclaim` CLI
# (docs/plan/06 §4, doc 10 M5).
#
# Produces a single fat binary (arm64 + x86_64) that links only system
# libraries, so it runs from a macOS Recovery Terminal or an external boot disk.
# There is no GUI in this crate; the release profile is optimized + thin-LTO.
# macOS forbids fully static executables (libSystem must be dynamic), so
# "static-ish" = static Rust std + only OS dylibs, verified by
# check-recovery-libs.sh.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"

VER="$(git describe --tags --always 2>/dev/null || echo dev)"
OUT="dist/reclaim-${VER}-universal-apple-darwin"
mkdir -p "$OUT"

TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
BINS=""
for t in $TARGETS; do
    echo "==> ensuring target $t"
    rustup target add "$t" >/dev/null 2>&1 || true
    echo "==> building reclaim for $t"
    cargo build --release -p reclaim-cli --target "$t"
    BINS="$BINS target/$t/release/reclaim"
done

echo "==> lipo → universal"
# shellcheck disable=SC2086
lipo -create $BINS -output "$OUT/reclaim"
chmod +x "$OUT/reclaim"

echo "==> ad-hoc codesign (Recovery accepts unnotarized; sign for good measure)"
codesign --force --sign - "$OUT/reclaim" 2>/dev/null || echo "   (codesign skipped)"

cp docs/recovery-mode.md "$OUT/README-recovery-mode.md" 2>/dev/null || true

echo "==> verifying linkage (system libs only)"
scripts/check-recovery-libs.sh "$OUT/reclaim"

echo ""
echo "built: $OUT/reclaim"
lipo -info "$OUT/reclaim" 2>/dev/null || true
echo "put it on a USB stick with:  scripts/recovery-usb.sh $OUT/reclaim /Volumes/<USB>"
