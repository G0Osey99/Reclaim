#!/usr/bin/env bash
# platform-proofs.sh — the four macOS platform proofs from Phase 0 (docs/plan/06,
# build guide Part 4 Phase 0 step F). Run WITH Ryker present because it needs
# sudo (raw device reads require root, and the boot device also needs Full Disk
# Access for the terminal app).
#
# It is READ-ONLY with respect to every disk. The only write it performs is
# creating (and then deleting) its own 100 MB temp file for the TRIM test.
#
# Usage:
#   sudo scripts/platform-proofs.sh                       # internal-only proofs
#   sudo scripts/platform-proofs.sh --ext-dev rdisk6 \    # external sacrificial device
#        --ext-mount /Volumes/RECLAIM_TEST                 # its mounted volume (for TRIM)
#
# Results are printed and appended to testdata/build/platform-proofs.txt.
set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$REPO/testdata/build/platform-proofs.txt"
mkdir -p "$REPO/testdata/build"

EXT_DEV=""
EXT_MOUNT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --ext-dev) EXT_DEV="$2"; shift 2 ;;
        --ext-mount) EXT_MOUNT="$2"; shift 2 ;;
        *) echo "unknown arg: $1"; exit 1 ;;
    esac
done

if [ "$(id -u)" != "0" ]; then
    echo "This script needs root. Re-run:  sudo $0 $*"
    exit 2
fi

log() { echo "$@" | tee -a "$OUT"; }
hr() { log "------------------------------------------------------------"; }

log ""
log "=== Reclaim platform proofs — $(date) ==="
log "host: $(scutil --get LocalHostName 2>/dev/null || hostname)  model: $(sysctl -n hw.model)  chip: $(sysctl -n machdep.cpu.brand_string 2>/dev/null)"
log "macOS: $(sw_vers -productVersion) ($(sw_vers -buildVersion))   SIP: $(csrutil status 2>/dev/null | sed 's/.*: //')   FileVault: $(fdesetup status 2>/dev/null | head -1)"

# Read the first `count` 512-byte sectors of a device and classify: does it look
# like an APFS structure (NXSB/APSB magic) or high-entropy (encrypted/random)?
classify_dev() {
    dev="$1"; count="${2:-2048}"
    tmp="$(mktemp)"
    if ! dd if="$dev" of="$tmp" bs=512 count="$count" 2>/dev/null; then
        echo "READ FAILED"
        rm -f "$tmp"
        return 1
    fi
    nxsb=$(LC_ALL=C grep -a -c "NXSB" "$tmp" 2>/dev/null || echo 0)
    apsb=$(LC_ALL=C grep -a -c "APSB" "$tmp" 2>/dev/null || echo 0)
    # crude entropy proxy: count distinct byte values in the first 64 KiB
    distinct=$(head -c 65536 "$tmp" | od -An -tu1 -v | tr ' ' '\n' | grep -c . )
    uniq=$(head -c 65536 "$tmp" | od -An -tu1 -v | tr ' ' '\n' | sort -u | grep -c .)
    rm -f "$tmp"
    echo "NXSB_hits=$nxsb APSB_hits=$apsb distinct_bytes_in_64k=$uniq/256"
}

# --- Proof (1): sudo raw read of an attached image + external sacrificial dev
hr; log "PROOF (1): raw /dev/rdiskN reads under sudo"
IMG="$REPO/testdata/build/exfat-camera-delete.img"
if [ -f "$IMG" ]; then
    DISK=$(hdiutil attach -nomount -imagekey diskimage-class=CRawDiskImage "$IMG" 2>/dev/null | awk 'NR==1{print $1}')
    if [ -n "$DISK" ]; then
        RDISK="/dev/r${DISK#/dev/}"
        if dd if="$RDISK" of=/dev/null bs=1m count=4 2>/dev/null; then
            log "  attached image $RDISK: raw read OK (4 MiB)"
        else
            log "  attached image $RDISK: raw read FAILED"
        fi
        hdiutil detach "$DISK" >/dev/null 2>&1
    else
        log "  could not attach $IMG"
    fi
else
    log "  (no golden image at $IMG — run scripts/gen-images first)"
fi
if [ -n "$EXT_DEV" ]; then
    if dd if="/dev/$EXT_DEV" of=/dev/null bs=1m count=4 2>/dev/null; then
        log "  external /dev/$EXT_DEV: raw read OK (4 MiB)"
    else
        log "  external /dev/$EXT_DEV: raw read FAILED"
    fi
else
    log "  external device: not provided (--ext-dev); plug in a sacrificial device to test."
fi

# --- Proof (2): boot physical store raw read (root + FDA)
hr; log "PROOF (2): boot physical store /dev/rdisk0 (needs root + Full Disk Access)"
if dd if=/dev/rdisk0 of=/dev/null bs=512 count=1 2>/dev/null; then
    log "  /dev/rdisk0 read OK -> running as root AND Full Disk Access effectively granted."
else
    log "  /dev/rdisk0 read DENIED -> root ok but Full Disk Access NOT granted to this terminal."
    log "    Fix: System Settings > Privacy & Security > Full Disk Access > enable your terminal, restart it."
fi

# --- Proof (3): FileVault-unlocked Data volume decrypted reads
hr; log "PROOF (3): decrypted reads of the unlocked Data volume"
log "  container /dev/rdisk3 : $(classify_dev /dev/rdisk3 4096)"
log "  Data vol /dev/rdisk3s5: $(classify_dev /dev/rdisk3s5 8192)"
log "  Interpretation: NXSB/APSB hits + low distinct-byte spread => decrypted APFS structures visible."
log "                  ~256/256 distinct bytes with no magic => encrypted/high-entropy (locked)."

# --- Proof (4): TRIM timing
hr; log "PROOF (4): TRIM timing (delete 100 MB, wait 60s, check extents read zeros)"
trim_test() {
    label="$1"; mount="$2"; dev="$3"
    f="$mount/.reclaim_trim_test.bin"
    log "  [$label] writing 100 MB to $f"
    if ! dd if=/dev/urandom of="$f" bs=1m count=100 2>/dev/null; then
        log "  [$label] could not write test file (skipping)"
        return
    fi
    sync
    # First extent (volume-relative device offset) via a tiny python helper.
    read -r devoff length < <(python3 - "$f" <<'PY'
import fcntl,os,struct,sys
F=65; fmt="<I4xqq"
fd=os.open(sys.argv[1],os.O_RDONLY)
buf=struct.pack(fmt,0,os.fstat(fd).st_size,0)
try:
    r=fcntl.fcntl(fd,F,buf); _,c,d=struct.unpack(fmt,r); print(d,c)
except OSError:
    print(-1,-1)
os.close(fd)
PY
)
    log "  [$label] first extent: dev_offset=$devoff length=$length (on $dev)"
    rm -f "$f"; sync
    log "  [$label] deleted; waiting 60 s for TRIM/discard..."
    sleep 60
    if [ "$devoff" -ge 0 ] 2>/dev/null; then
        zeros=$(dd if="$dev" bs=1 skip="$devoff" count=4096 2>/dev/null | od -An -tu1 | tr ' ' '\n' | grep -v '^$' | grep -vc '^0$')
        if [ "$zeros" -eq 0 ]; then
            log "  [$label] extent now reads ZEROS -> TRIM happened (deleted data gone)."
        else
            log "  [$label] extent still has $zeros non-zero bytes in first 4 KiB -> data recoverable."
        fi
    else
        log "  [$label] extent mapping unavailable on this filesystem (skip raw check)."
    fi
}
trim_test "internal" "$HOME" "/dev/rdisk3s5"
if [ -n "$EXT_MOUNT" ] && [ -n "$EXT_DEV" ]; then
    trim_test "external" "$EXT_MOUNT" "/dev/r$EXT_DEV"
else
    log "  external SSD / SD card: not provided; plug one in and pass --ext-dev/--ext-mount."
fi

hr; log "Proofs complete. Full log: $OUT"
