# Running Reclaim from macOS Recovery

When the disk you want to recover is the Mac's **internal boot disk**, the safest
way to scan it is to *stop using that Mac* and run Reclaim from an environment
that is not writing to the disk. macOS Recovery is exactly that: a minimal system
booted from a separate partition, with a Terminal, that touches the Data volume
only if you ask it to. Reclaim ships a single self-contained CLI binary that runs
there unchanged — no installer, no GUI frameworks, no third-party libraries
(docs/plan/06 §4).

> **SIP stays on.** Nothing here disables System Integrity Protection
> (`csrutil` is never run). Reading raw devices does not require SIP off; only
> writing to system locations does, which Reclaim never does.

## 1. Build the binary (on a working Mac)

```bash
scripts/build-recovery.sh
```

This produces a universal (arm64 + x86_64) `dist/reclaim-<ver>-universal-apple-darwin/reclaim`.
`scripts/check-recovery-libs.sh` confirms it links **only** system libraries
(`/usr/lib/**`, `/System/Library/**`), so it runs in Recovery where nothing else
is installed:

```
/System/Library/Frameworks/IOKit.framework/.../IOKit
/System/Library/Frameworks/CoreFoundation.framework/.../CoreFoundation
/usr/lib/libiconv.2.dylib
/usr/lib/libSystem.B.dylib
```

## 2. Put it on a USB stick

Plug in a USB stick that has room for the binary **and** for the files you expect
to recover (recover to the stick or another external drive — never back to the
source). Then:

```bash
scripts/recovery-usb.sh dist/reclaim-<ver>-universal-apple-darwin/reclaim /Volumes/<USB>
```

The script only *copies* onto an already-mounted volume; it never erases a disk.
To prepare a blank stick yourself first:

```bash
diskutil list                                  # find the USB's diskN
diskutil eraseVolume ExFAT RECLAIM /dev/diskN  # ERASES that disk
```

## 3. Boot into Recovery

- **Apple silicon:** shut down, then press and hold the **power button** until
  “Loading startup options” appears → **Options** → **Continue**.
- **Intel:** power on and immediately hold **⌘‑R**.

Pick a user and enter the password if prompted. From the menu bar choose
**Utilities → Terminal**.

## 4. Run Reclaim

The USB stick auto-mounts under `/Volumes`. List what's attached, then scan:

```sh
ls /Volumes
/Volumes/RECLAIM/reclaim-recovery/reclaim list
/Volumes/RECLAIM/reclaim-recovery/reclaim info disk0
```

Identify the **Data** volume of the internal disk (its content is on the
container's Data role — `reclaim list` shows mounts and roles). Then scan it and
recover **to the USB stick**:

```sh
cd /Volumes/RECLAIM/reclaim-recovery
./reclaim scan disk3s5 --session /Volumes/RECLAIM/session
./reclaim results /Volumes/RECLAIM/session --deleted-only
./reclaim recover /Volumes/RECLAIM/session /Volumes/RECLAIM/Recovered --deleted-only --verify
```

Reclaim refuses a destination on the same physical disk as the source, so it will
not let you recover back onto the disk you are reading.

### Notes for Recovery Mode

- **Permissions.** Raw device nodes are `root:operator 0640`. In Recovery the
  Terminal already runs as root, so `reclaim` can open `/dev/rdiskN` directly —
  no `sudo`, no Full Disk Access dance (those apply to a normal login; docs/plan/06 §12).
- **FileVault.** If the internal volume is encrypted, unlock it first from
  **Disk Utility** (or `diskutil apfs unlockVolume <uuid>`); Reclaim then reads
  the OS‑decrypted volume. It does not implement its own FileVault crypto
  (docs/plan/06 §5).
- **A dying drive.** `reclaim info diskN` prints a health verdict; if it is
  *marginal*/*FAILING*, image the disk to the USB first and scan the image:
  `./reclaim image disk3 /Volumes/RECLAIM/disk3.img --retries 5 --reverse-pass`
  then `./reclaim scan /Volumes/RECLAIM/disk3.img`.
- **Interrupted scans resume.** If the run is stopped (or the source is
  unplugged), re-run the same `scan … --resume`; the session re-identifies the
  source by its first/last-MiB hashes (docs/plan/07 §4).

## 5. Alternative: an external boot disk

For repeated use, install macOS onto an external SSD, copy the universal
`reclaim` binary onto it, and boot the target Mac from that disk (hold the power
button / ⌘‑R equivalent and pick the external disk). This gives a full shell with
Reclaim preinstalled while leaving the internal disk unmounted for scanning.
