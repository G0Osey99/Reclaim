#!/usr/bin/env bash
# platform-proofs.sh — the four macOS platform proofs from Phase 0 (docs/plan/06
# §12, build guide Part 4 step F).
#
# IMPORTANT — two independent gates protect the internal raw devices, and they
# pull in opposite directions (learned the hard way in the first Phase-0 run):
#   * DAC (Unix perms): /dev/rdisk* are crw-r----- root:operator, so opening
#     them needs root OR membership in the `operator` group.
#   * TCC (Full Disk Access): the boot/container/Data devices are TCC-protected.
#     FDA is attributed to the shell's *responsible process*; running under
#     `sudo` re-parents the process and DROPS that attribution, so a sudo'd read
#     of an internal device gets EPERM even though root clears DAC.
# => The reliable way to read the internal devices is NON-sudo, as a user who is
#    in the `operator` group and whose terminal app holds Full Disk Access:
#        sudo dseditgroup -o edit -a "$USER" -t user operator   # one-time
#        # open a fresh terminal session, then:
#        scripts/platform-proofs.sh                             # NO sudo
#
# It is READ-ONLY with respect to every disk. The only write it performs is
# creating (and then deleting) its own temp file for the internal TRIM probe.
#
# Usage:
#   scripts/platform-proofs.sh                                  # internal proofs
#   scripts/platform-proofs.sh --ext-dev disk6 --ext-mount /Volumes/RECLAIM_TEST
#
# Written for bash 3.2 (stock macOS).
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
# Normalize an external device id to a bare "diskN" (strip /dev/ and leading r).
EXT_DEV="${EXT_DEV#/dev/}"; EXT_DEV="${EXT_DEV#r}"

log() { echo "$@" | tee -a "$OUT"; }
hr() { log "------------------------------------------------------------"; }

# External TRIM erasure timing on a filesystem we own end-to-end (defined early
# so Proof 4 can call it). Writes+deletes only its own ~100 MB temp file.
trim_external() {
    mount="$1"; dev="$2"; f="$mount/.reclaim_trim_test.bin"
    log "  [external] writing 100 MB to $f"
    if ! dd if=/dev/urandom of="$f" bs=1m count=100 >/dev/null 2>&1; then
        log "  [external] could not write test file (skip)"; return
    fi
    sync
    read -r devoff length < <(python3 - "$f" <<'PY'
import fcntl,os,struct,sys
F=65; fmt="<I4xqq"
fd=os.open(sys.argv[1],os.O_RDONLY); sz=os.fstat(fd).st_size
try:
    _,c,d=struct.unpack(fmt,fcntl.fcntl(fd,F,struct.pack(fmt,0,sz,0)))
    print(d,c) if (0<=d and 0<c<=sz) else print(-1,-1)
except OSError: print(-1,-1)
os.close(fd)
PY
)
    if [ "${devoff:--1}" -lt 0 ] 2>/dev/null; then
        log "  [external] extent mapping unavailable on this FS -> cannot verify erasure"; rm -f "$f"; return
    fi
    # Baseline: the extent should hold our data BEFORE deletion.
    b=$(read_dev "$dev" 8); case "$b" in OK*) rm -f "$(echo "$b" | awk '{print $3}')";; esac
    log "  [external] first extent dev_offset=$devoff len=$length; deleting + waiting 60s"
    rm -f "$f"; sync; sleep 60
    nread=$(dd if="$dev" bs=1 skip="$devoff" count=4096 2>/dev/null | wc -c | tr -d ' ')
    z=$(dd if="$dev" bs=1 skip="$devoff" count=4096 2>/dev/null | tr -d '\000' | wc -c | tr -d ' ')
    if [ "$nread" != "4096" ]; then
        log "  [external] raw read denied/short ($nread bytes) -> INCONCLUSIVE"
    elif [ "$z" = "0" ]; then
        log "  [external] extent reads ZEROS -> TRIM/discard happened (deleted data gone)"
    else
        log "  [external] extent still has $z non-zero bytes -> data recoverable (no TRIM)"
    fi
}

AM_ROOT=0; [ "$(id -u)" = "0" ] && AM_ROOT=1
IN_OPERATOR=0; id -Gn | tr ' ' '\n' | grep -qx operator && IN_OPERATOR=1

# FDA canary: can this process read a TCC-protected file? (Read-only.)
fda_effective() {
    dd if="$HOME/Library/Messages/chat.db" of=/dev/null bs=1 count=16 >/dev/null 2>&1
}
FDA_OK=0; fda_effective && FDA_OK=1

# Read `count` 512-byte sectors of a device; classify the failure by errno.
# Prints "OK <bytes>" on success, or "FAIL <reason>" on error.
read_dev() {
    dev="$1"; count="${2:-1}"; tmp="$(mktemp)"; err="$(mktemp)"
    dd if="$dev" of="$tmp" bs=512 count="$count" >/dev/null 2>"$err"
    st=$?
    n=$(wc -c < "$tmp" | tr -d ' ')
    msg="$(cat "$err")"
    rm -f "$err"
    if [ "$st" = "0" ] && [ "$n" -gt 0 ]; then
        echo "OK $n $tmp"
        return 0
    fi
    rm -f "$tmp"
    case "$msg" in
        *"Operation not permitted"*) echo "FAIL TCC (Operation not permitted — FDA not effective for this process; sudo drops it)";;
        *"Permission denied"*)       echo "FAIL DAC (Permission denied — need root or operator-group membership)";;
        *"Resource busy"*)           echo "FAIL BUSY (Resource busy — device opened exclusively)";;
        *)                            echo "FAIL OTHER (${msg:-status $st})";;
    esac
    return 1
}

# Classify a captured block file: APFS structures (NXSB/APSB) vs all-zero vs
# high-entropy ciphertext.
classify_file() {
    f="$1"
    nxsb=$(LC_ALL=C grep -a -c "NXSB" "$f"); nxsb=${nxsb:-0}
    apsb=$(LC_ALL=C grep -a -c "APSB" "$f"); apsb=${apsb:-0}
    nzero=$(head -c 65536 "$f" | tr -d '\000' | wc -c | tr -d ' ')
    uniq=$(head -c 65536 "$f" | od -An -tu1 -v | tr ' ' '\n' | grep -v '^$' | sort -u | grep -c .)
    if [ "$nzero" = "0" ]; then
        echo "ALL-ZERO (no data / trimmed) [distinct=$uniq/256]"
    elif [ "$nxsb" -gt 0 ] || [ "$apsb" -gt 0 ]; then
        echo "DECRYPTED APFS (NXSB=$nxsb APSB=$apsb, distinct=$uniq/256)"
    elif [ "$uniq" -ge 250 ]; then
        echo "HIGH-ENTROPY / ciphertext (no APFS magic, distinct=$uniq/256)"
    else
        echo "structured, non-APFS-magic (distinct=$uniq/256)"
    fi
}

log ""
log "=== Reclaim platform proofs — $(date) ==="
log "host: $(scutil --get LocalHostName 2>/dev/null || hostname)  model: $(sysctl -n hw.model)  chip: $(sysctl -n machdep.cpu.brand_string 2>/dev/null)"
log "macOS: $(sw_vers -productVersion) ($(sw_vers -buildVersion))   SIP: $(csrutil status 2>/dev/null | sed 's/.*: //')   FileVault: $(fdesetup status 2>/dev/null | head -1)"
log "context: root=$AM_ROOT  operator-group=$IN_OPERATOR  FDA-effective=$FDA_OK"
if [ "$AM_ROOT" = "1" ]; then
    log "WARNING: running under sudo/root. Internal TCC-protected devices (rdisk0/"
    log "         container/Data) will likely FAIL here because sudo drops the FDA"
    log "         attribution. For proofs 2 & 3, run this WITHOUT sudo as an"
    log "         operator-group member (see the header)."
fi
if [ "$AM_ROOT" = "0" ] && [ "$IN_OPERATOR" = "0" ]; then
    log "NOTE: not root and not in the operator group -> internal device opens will"
    log "      fail on DAC. One-time fix: sudo dseditgroup -o edit -a \"$USER\" -t user operator,"
    log "      then open a fresh terminal session and re-run WITHOUT sudo."
fi

# Runtime device resolution (never hardcode synthesized disk numbers).
DATA_DEV=$(diskutil info -plist /System/Volumes/Data 2>/dev/null | plutil -extract DeviceIdentifier raw - 2>/dev/null)
CONTAINER=$(diskutil info -plist /System/Volumes/Data 2>/dev/null | plutil -extract APFSContainerReference raw - 2>/dev/null)
PSTORE=$(diskutil info -plist "${CONTAINER:-disk3}" 2>/dev/null | plutil -extract APFSPhysicalStores.0.DeviceIdentifier raw - 2>/dev/null)
BOOT_WHOLE=$(diskutil info -plist "${PSTORE:-disk0s2}" 2>/dev/null | plutil -extract ParentWholeDisk raw - 2>/dev/null)
DATA_DEV="${DATA_DEV:-disk3s5}"; CONTAINER="${CONTAINER:-disk3}"; PSTORE="${PSTORE:-disk0s2}"; BOOT_WHOLE="${BOOT_WHOLE:-disk0}"
log "resolved: boot-whole=$BOOT_WHOLE  container=$CONTAINER  physical-store=$PSTORE  data-volume=$DATA_DEV"

# --- Proof (1): raw read of an attached image (not TCC-protected) + external
hr; log "PROOF (1): raw /dev/rdiskN reads (attached image + optional external)"
IMG="$REPO/testdata/build/exfat-camera-delete.img"
if [ -f "$IMG" ]; then
    DISK=$(hdiutil attach -nomount -imagekey diskimage-class=CRawDiskImage "$IMG" 2>/dev/null | awk 'NR==1{print $1}')
    if [ -n "$DISK" ]; then
        RDISK="/dev/r${DISK#/dev/}"
        r=$(read_dev "$RDISK" 8192); log "  attached image $RDISK: ${r%% /*}"
        case "$r" in OK*) rm -f "$(echo "$r" | awk '{print $3}')";; esac
        hdiutil detach "$DISK" >/dev/null 2>&1
    else
        log "  could not attach $IMG"
    fi
else
    log "  (no golden image at $IMG — run scripts/gen-images first)"
fi
if [ -n "$EXT_DEV" ]; then
    r=$(read_dev "/dev/r$EXT_DEV" 8192); log "  external /dev/r$EXT_DEV: ${r%% /*}"
    case "$r" in OK*) rm -f "$(echo "$r" | awk '{print $3}')";; esac
else
    log "  external device: not provided (--ext-dev). NOTE: a raw read is"
    log "    non-destructive, so ANY external disk works here — it need not be erasable."
fi

# --- Proof (2): boot physical store (root+FDA, or operator+FDA non-sudo)
hr; log "PROOF (2): boot physical store /dev/r$BOOT_WHOLE"
r=$(read_dev "/dev/r$BOOT_WHOLE" 1)
log "  ${r%% /*}"
case "$r" in
    OK*) log "  -> raw read of the boot physical store SUCCEEDED (both gates satisfied)."
         rm -f "$(echo "$r" | awk '{print $3}')";;
    *TCC*) log "  -> FDA attribution lost (you are under sudo). Re-run non-sudo as operator.";;
    *DAC*) log "  -> add \$USER to the operator group and re-run non-sudo (see header).";;
esac

# --- Proof (3): decrypted reads of the unlocked, encrypted Data volume
hr; log "PROOF (3): decrypted reads of the unlocked Data volume (Encryption=true, FileVault=true)"
rc=$(read_dev "/dev/r$CONTAINER" 4096)
if [ "${rc%% *}" = "OK" ]; then
    f=$(echo "$rc" | awk '{print $3}'); log "  container /dev/r$CONTAINER : $(classify_file "$f")"; rm -f "$f"
else log "  container /dev/r$CONTAINER : ${rc%% /*}"; fi
rp=$(read_dev "/dev/r$PSTORE" 4096)
if [ "${rp%% *}" = "OK" ]; then
    f=$(echo "$rp" | awk '{print $3}'); log "  phys-store /dev/r$PSTORE : $(classify_file "$f")"; rm -f "$f"
else log "  phys-store /dev/r$PSTORE : ${rp%% /*}"; fi
rd=$(read_dev "/dev/r$DATA_DEV" 8192)
if [ "${rd%% *}" = "OK" ]; then
    f=$(echo "$rd" | awk '{print $3}'); log "  data vol  /dev/r$DATA_DEV : $(classify_file "$f")"; rm -f "$f"
else log "  data vol  /dev/r$DATA_DEV : ${rd%% /*}"; fi
log "  OPEN QUESTION: on Apple Silicon the SEP decrypts transparently when booted"
log "  & unlocked, so a successful read is EXPECTED to show DECRYPTED APFS (NXSB on"
log "  the container/physical-store, APSB on the volume) despite Encryption=true. A"
log "  HIGH-ENTROPY result would mean the raw node returns ciphertext. This is the"
log "  doc 06 §4 question — record whichever classify_file reports above."

# --- Proof (4): TRIM
hr; log "PROOF (4): TRIM"
log "  internal SSD TRIM CAPABILITY (authoritative, no sudo/media):"
log "    $(system_profiler SPNVMeDataType 2>/dev/null | grep -i 'TRIM Support' | head -1 | sed 's/^ *//')"
log "  internal ERASURE-TIMING behavioral test is NOT done here: on the internal"
log "  FileVault APFS volume F_LOG2PHYS extent->raw-offset mapping is unreliable"
log "  (COW + local snapshots can retain deleted blocks), so a clean write->delete->"
log "  read-zero measurement needs an UNENCRYPTED external device you control."
if [ -n "$EXT_DEV" ] && [ -n "$EXT_MOUNT" ]; then
    trim_external "$EXT_MOUNT" "/dev/r$EXT_DEV"
else
    log "  external SSD / SD-card erasure timing: provide --ext-dev/--ext-mount"
    log "    (needs ~100 MB free; the temp file is created and deleted — a fully"
    log "     erasable drive is only required for whole-device wipe verification)."
fi

hr; log "Proofs complete. Full log: $OUT"
