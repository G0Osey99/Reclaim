#!/usr/bin/env bash
# recovery-usb.sh — stage the Recovery-Mode `reclaim` CLI onto a USB stick
# (docs/plan/06 §4, doc 10 M5).
#
# Non-destructive: it COPIES the universal binary + the recovery instructions to
# an already-mounted volume. It never erases or partitions anything — formatting
# a stick is a one-line `diskutil` command the operator runs themselves (printed
# below), so this script cannot wipe the wrong disk.
#
# Usage:
#   scripts/recovery-usb.sh [BINARY] [/Volumes/<USB>]
# with no args it builds the universal binary first and lists candidate volumes.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

BIN="${1:-}"
DEST="${2:-}"

if [ -z "$BIN" ]; then
    echo "building the universal recovery binary first…"
    scripts/build-recovery.sh
    BIN="$(ls -1 dist/reclaim-*-universal-apple-darwin/reclaim 2>/dev/null | tail -1)"
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
    echo "error: no recovery binary at '$BIN' — run scripts/build-recovery.sh first." >&2
    exit 1
fi

if [ -z "$DEST" ]; then
    echo ""
    echo "no destination volume given. Mounted volumes you could copy to:"
    ls -1 /Volumes 2>/dev/null | sed 's/^/  \/Volumes\//'
    echo ""
    echo "To prepare a fresh USB stick (ERASES it — run it yourself):"
    echo "  diskutil list                       # find the USB's diskN"
    echo "  diskutil eraseVolume ExFAT RECLAIM /dev/diskN"
    echo "then re-run:  scripts/recovery-usb.sh '$BIN' /Volumes/RECLAIM"
    exit 0
fi

if [ ! -d "$DEST" ]; then
    echo "error: destination '$DEST' is not a mounted volume." >&2
    exit 1
fi

# Refuse to write onto the boot volume / its data volume.
case "$DEST" in
    "/" | "/System/Volumes/Data" | "/System/Volumes/Data/"*)
        echo "error: refusing to write to the boot volume '$DEST'." >&2
        exit 1 ;;
esac

echo "copying recovery kit → $DEST/reclaim-recovery/"
mkdir -p "$DEST/reclaim-recovery"
cp "$BIN" "$DEST/reclaim-recovery/reclaim"
chmod +x "$DEST/reclaim-recovery/reclaim"
[ -f docs/recovery-mode.md ] && cp docs/recovery-mode.md "$DEST/reclaim-recovery/README-recovery-mode.md"

echo "done. In a Recovery Terminal:"
echo "  /Volumes/<thisUSB>/reclaim-recovery/reclaim list"
echo "  /Volumes/<thisUSB>/reclaim-recovery/reclaim scan diskN"
echo "(recover TO the USB or another external drive, never back to the source)."
