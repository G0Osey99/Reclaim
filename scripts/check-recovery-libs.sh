#!/usr/bin/env bash
# check-recovery-libs.sh — Recovery-Mode linkage gate (docs/plan/06 §4, doc 10 M5).
#
# The `reclaim` CLI must run from a macOS Recovery Terminal, where only the OS's
# own libraries exist. This asserts the release binary links *only* system
# libraries (`/usr/lib/**`, `/System/Library/**`) — no Homebrew dylibs, no
# @rpath third-party loads. It is macOS-only (uses otool); a no-op elsewhere.
#
# Written for bash 3.2 (stock macOS).
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "check-recovery-libs: not macOS — skipping (otool unavailable)."
    exit 0
fi

BIN="${1:-target/release/reclaim}"
if [ ! -x "$BIN" ]; then
    echo "check-recovery-libs: building release binary…"
    export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
    cargo build --release -p reclaim-cli >/dev/null
fi

echo "check-recovery-libs: otool -L $BIN"
# Real dependency lines carry "(compatibility version …)"; the per-file /
# per-architecture header lines (the binary's own path) do not, so filtering on
# that marker works for both thin and fat (universal) binaries.
LIBS="$(otool -L "$BIN" | grep -E '\(compatibility version' | awk '{print $1}' | sort -u)"

bad=0
while IFS= read -r lib; do
    [ -z "$lib" ] && continue
    case "$lib" in
        /usr/lib/*|/System/Library/*)
            : ;; # system library — OK in Recovery Mode
        *)
            echo "  NON-SYSTEM LIB: $lib"
            bad=$((bad + 1)) ;;
    esac
done <<EOF
$LIBS
EOF

if [ "$bad" -gt 0 ]; then
    echo ""
    echo "check-recovery-libs: $bad non-system library link(s) — the recovery build"
    echo "would not run from a Recovery Terminal. Keep all deps pure-Rust or"
    echo "statically linked (build guide Part 1.4 / docs/plan/06 §4)."
    exit 1
fi

echo "$LIBS" | sed 's/^/  ok: /'
echo "check-recovery-libs: OK — only system libraries linked."
