#!/usr/bin/env bash
# gen-p4-golden.sh — build the Phase-4 golden images (docs/plan/09 §2).
#
# Reproduces the seven Phase-4 recovery cases in testdata/build/ (gitignored):
#   ext4-delete, ext2-delete        — real mke2fs images, files removed with debugfs
#   ntfs-in-vmdk                    — ntfs-delete.img wrapped in a VMDK (qemu-img)
#   apfs-in-dmg                     — apfs-many-deletes.img wrapped in a UDZO DMG
#   exfat-zeroed-boot              — exfat-camera-delete.img, main VBR zeroed
#   hfs-zeroed-vh                  — hfsplus-delete.img, main volume header zeroed
# (gpt-deleted-partition is built by scripts/gen-fs-images.)
#
# Requires: e2fsprogs (mke2fs/debugfs), qemu-img, hdiutil, python3. Each step is
# skipped with a note if its inputs/tools are missing.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="/opt/homebrew/opt/e2fsprogs/sbin:/opt/homebrew/bin:$PATH"

B=testdata/build
mkdir -p "$B"

have() { command -v "$1" >/dev/null 2>&1; }

# --- ext2/ext4 deleted-file images (mke2fs -d + debugfs rm) ----------------
gen_ext() {
    local fstype="$1" out="$2"
    if ! have mke2fs || ! have debugfs; then
        echo "skip $out: e2fsprogs not found (brew install e2fsprogs)"; return
    fi
    local src="$B/.ext-src"
    rm -rf "$src"; mkdir -p "$src/DCIM"
    python3 - "$src" <<'PY'
import os,sys
r=sys.argv[1]
for i in range(8):
    open(f"{r}/file_{i:03d}.txt","wb").write((f"golden {i}\n").encode()*(200+i*13))
open(f"{r}/DCIM/IMG_0001.jpg","wb").write(b"\xff\xd8\xff\xe0"+b"J"*8000+b"\xff\xd9")
PY
    rm -f "$out"
    mke2fs -q -F -t "$fstype" -b 1024 -d "$src" "$out" 8192 2>/dev/null
    for f in file_000.txt file_002.txt DCIM/IMG_0001.jpg; do
        debugfs -w -R "rm /$f" "$out" >/dev/null 2>&1
    done
    echo "built $out ($fstype, deleted file_000.txt file_002.txt DCIM/IMG_0001.jpg)"
}
gen_ext ext4 "$B/ext4-delete.img"
gen_ext ext2 "$B/ext2-delete.img"

# ext ground-truth sidecars (deterministic content above) so scripts/bench.sh can
# score them. 8 txt + 1 jpg populated; file_000/002 + IMG_0001.jpg are deleted.
for pair in ext4:ext4-delete ext2:ext2-delete; do
    fstype="${pair%%:*}"; name="${pair##*:}"
    [ -f "$B/$name.img" ] || continue
    python3 - "$fstype" "$B/$name.groundtruth.json" <<'PY'
import hashlib, json, sys, time, socket
fstype, out = sys.argv[1], sys.argv[2]
h = lambda b: hashlib.sha256(b).hexdigest()
files = []
for i in range(8):
    b = (f"golden {i}\n").encode() * (200 + i * 13)
    files.append({"path": f"file_{i:03d}.txt", "family": "txt", "size": len(b),
                  "sha256": h(b), "extents": [], "deleted": i in (0, 2)})
jb = b"\xff\xd8\xff\xe0" + b"J" * 8000 + b"\xff\xd9"
files.append({"path": "DCIM/IMG_0001.jpg", "family": "jpeg", "size": len(jb),
              "sha256": h(jb), "extents": [], "deleted": True})
json.dump({"recipe": out, "fs": fstype, "scheme": "none", "size_mb": 8,
           "built_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "host": socket.gethostname(),
           "note": "deterministic content from gen_ext (mke2fs -d + debugfs rm)",
           "files": files}, open(out, "w"), indent=1)
PY
    echo "wrote $name.groundtruth.json"
done

# --- ntfs-in-vmdk ----------------------------------------------------------
if have qemu-img && [ -f "$B/ntfs-delete.img" ]; then
    rm -f "$B/ntfs-in-vmdk.vmdk"
    qemu-img convert -f raw -O vmdk "$B/ntfs-delete.img" "$B/ntfs-in-vmdk.vmdk" && \
        echo "built $B/ntfs-in-vmdk.vmdk (from ntfs-delete.img)"
    # The container presents the guest image; reuse the guest's ground truth.
    [ -f "$B/ntfs-delete.groundtruth.json" ] && cp "$B/ntfs-delete.groundtruth.json" "$B/ntfs-in-vmdk.groundtruth.json"
else
    echo "skip ntfs-in-vmdk: need qemu-img + ntfs-delete.img (scripts/gen-fs-images ntfs-delete)"
fi

# --- apfs-in-dmg -----------------------------------------------------------
if have hdiutil && [ -f "$B/apfs-many-deletes.img" ]; then
    rm -f "$B/apfs-in-dmg.dmg"
    hdiutil convert "$B/apfs-many-deletes.img" -format UDZO -o "$B/apfs-in-dmg" >/dev/null && \
        echo "built $B/apfs-in-dmg.dmg (UDZO/zlib, from apfs-many-deletes.img)"
    [ -f "$B/apfs-many-deletes.groundtruth.json" ] && cp "$B/apfs-many-deletes.groundtruth.json" "$B/apfs-in-dmg.groundtruth.json"
else
    echo "skip apfs-in-dmg: need hdiutil + apfs-many-deletes.img (scripts/gen-images)"
fi

# --- exfat-zeroed-boot (zero the main VBR; backup boot survives) -----------
if [ -f "$B/exfat-camera-delete.img" ]; then
    cp "$B/exfat-camera-delete.img" "$B/exfat-zeroed-boot.img"
    python3 - "$B/exfat-zeroed-boot.img" <<'PY'
import sys
f=sys.argv[1]; d=bytearray(open(f,"rb").read())
i=d.find(b"EXFAT   "); vol=i-3
for k in range(vol, vol+512): d[k]=0
open(f,"wb").write(d); print(f"built {f} (main VBR at {hex(vol)} zeroed)")
PY
    # Same guest content as exfat-camera-delete; recovery goes via the backup boot.
    [ -f "$B/exfat-camera-delete.groundtruth.json" ] && cp "$B/exfat-camera-delete.groundtruth.json" "$B/exfat-zeroed-boot.groundtruth.json"
else
    echo "skip exfat-zeroed-boot: need exfat-camera-delete.img"
fi

# --- hfs-zeroed-vh (zero the main volume header; alt VH survives) ----------
if [ -f "$B/hfsplus-delete.img" ]; then
    cp "$B/hfsplus-delete.img" "$B/hfs-zeroed-vh.img"
    python3 - "$B/hfs-zeroed-vh.img" <<'PY'
import sys
f=sys.argv[1]; d=bytearray(open(f,"rb").read())
i=d.find(b"H+")            # main VH at volume+1024
for k in range(i, i+512): d[k]=0
open(f,"wb").write(d); print(f"built {f} (main VH at {hex(i)} zeroed)")
PY
    # Same guest content as hfsplus-delete; recovery goes via the alternate VH.
    [ -f "$B/hfsplus-delete.groundtruth.json" ] && cp "$B/hfsplus-delete.groundtruth.json" "$B/hfs-zeroed-vh.groundtruth.json"
else
    echo "skip hfs-zeroed-vh: need hfsplus-delete.img"
fi

echo "done."
