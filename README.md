# Reclaim

**Read-only disk, file, and photo recovery for macOS — with names, previews, and published numbers.**

Reclaim recovers deleted and lost files from disks, SD cards, USB drives, and disk
images. It is as safe as PhotoRec (it never writes to the media it reads), recovers
**original names and folders** the way the commercial Mac tools do, previews files
before you recover them, and — unlike every commercial tool — **publishes its
recovery-rate methodology and its full benchmark against PhotoRec** (see
[docs/benchmarks.md](docs/benchmarks.md)).

It ships as a single self-contained CLI (`reclaim`) that also runs from macOS
Recovery, and a native SwiftUI app (`Reclaim.app`).

- **Core & CLI:** Apache-2.0, no GPL code copied.
- **Platforms:** macOS 13+ (Apple Silicon & Intel, universal). The CLI also builds for Linux.
- **What it is not:** a filesystem *repair* tool, and not a NIST-CFTT-certified forensic tool (see [Forensic soundness](#forensic-soundness)).

---

## Safety promises

These are guarantees, enforced by construction and by CI, not marketing (see
[docs/plan/11-safety-legal-licensing.md](docs/plan/11-safety-legal-licensing.md) §1):

1. **Read-only sources.** No code path can write to a scanned device. The block
   layer exposes no write method to any engine; a CI grep denies write syscalls
   outside the imager destination and the session store; every integration test
   hashes its source image before *and* after and fails on any change. Run
   `reclaim --prove-readonly <cmd>` to print every `open()` with its flags.
2. **Never recover onto the source.** Reclaim refuses a destination on the same
   physical disk as the source. The override flag is deliberately long and ugly
   (`--allow-same-device-i-accept-data-loss`).
3. **Image-first on unhealthy media.** `reclaim info` prints a health verdict; a
   marginal/failing drive is imaged first (the app forces this flow, the CLI warns).
4. **Session durability.** A crash or an unplug loses at most one checkpoint
   interval; re-run `scan --resume`.
5. **Honesty about chances.** TRIM'd SSDs, overwritten extents, and encrypted
   volumes are labeled. There is no "100% recovery" claim anywhere. See below.
6. **No telemetry.** The core and CLI phone home to nobody. Recovered content and
   session databases stay on your machine (`0700` session dirs).

---

## Will my files come back? — honest chances by media

Recovery depends almost entirely on whether the freed blocks have been reused or
discarded. The single most important fact for Mac users
([docs/plan/06-macos-platform-notes.md](docs/plan/06-macos-platform-notes.md) §3):

| Media | Deleted-file chances | Why |
|---|---|---|
| **Internal Apple SSD** (the Mac's own disk) | **Low for plain deletes** | APFS issues TRIM/discard on free; freed blocks read back as zeros within seconds. Carving deleted content is mostly futile. |
| Internal SSD — still recoverable paths | Medium | APFS **snapshots** and **checkpoint history** (blocks still referenced by an older transaction are not freed → not trimmed), Time Machine local snapshots, and files still in `.Trash`. Reclaim reads all of these. |
| **SD cards, USB sticks, camera media** | **High** | No TRIM over USB mass storage; cameras don't TRIM. Freed blocks survive until overwritten. This is where carving shines. |
| **External HDDs** | **High** | No TRIM; magnetic platters retain data until rewritten. |
| Encrypted volume (FileVault / BitLocker) | Only if unlocked | Reclaim reads the OS-unlocked device; it never brute-forces a passphrase. |

**Golden rule:** the moment you realize something is lost, **stop using that
disk.** Every write reduces your chances. For the Mac's own boot disk, that means
running Reclaim from [Recovery mode](docs/recovery-mode.md) or from another Mac.

---

## Install

### Homebrew (recommended)

```bash
# CLI
brew install G0Osey99/reclaim/reclaim

# App
brew install --cask G0Osey99/reclaim/reclaim
```

### Download

Grab the notarized `Reclaim-1.0.0.dmg` (app) and the `reclaim-1.0.0-*.tar.gz`
(CLI) from the [latest release](https://github.com/G0Osey99/reclaim/releases/latest),
verify against `SHA256SUMS`, and drag the app to `/Applications`.

### From source

```bash
git clone https://github.com/G0Osey99/reclaim
cd reclaim
cargo build --release -p reclaim-cli     # -> target/release/reclaim
```

Building the app needs Xcode; see [scripts/build-app.sh](scripts/build-app.sh).

---

## 60-second tutorial

### CLI

```bash
# 1. See what's attached (no root needed for this).
reclaim list

# 2. Look at one source — size, filesystem, health verdict, recovery chances.
reclaim info disk4

# 3. Scan an SD card into a session (metadata + carving; reads only).
#    Raw devices need root: run under sudo, or from Recovery (already root).
sudo reclaim scan disk4 --session ~/reclaim-sd

# 4. Browse what was found — filter to deleted files only.
reclaim results ~/reclaim-sd --deleted-only

# 5. Recover to ANOTHER disk, preserving folders, verifying each file.
#    (Reclaim refuses to recover back onto the source.)
reclaim recover ~/reclaim-sd /Volumes/Backup/Recovered --deleted-only --preserve-paths --verify
```

Scanning a disk image needs no root at all:

```bash
reclaim scan card.dmg --session ~/reclaim-card   # .dmg/.img/.iso/E01/VMDK/... auto-detected
```

Every command takes `--json` for NDJSON output and returns a documented exit code
— see the full [CLI reference](docs/cli.md).

### App

1. Open **Reclaim.app**. On first launch it walks you through installing the
   read-only privileged helper (one click → approve in **System Settings → Login
   Items**) and granting **Full Disk Access**.
2. Pick a source in the sidebar. A red health chip swaps the flow to **Image first**.
3. Press scan. Files stream in with thumbnails *while the scan runs*; filter by
   type, recoverability, or "deleted only" on the left; preview on the right.
4. Select files → **Recover** to a different disk, with verify-after-copy. Done.

---

## What it recovers

**Engines (run in one sequential pass over the media):**
- **Metadata recovery** — parse the filesystem, list live *and* deleted entries
  with their real names, paths, timestamps, and extents.
- **Signature carving** — 140 signatures spanning **210 file extensions** across
  images, camera RAW (22 formats), video, audio, documents, archives, databases,
  and more, each with format-aware validation to reject false positives.
- **Lost-structure search** — find boot sectors, superblocks, APFS
  checkpoints/volume superblocks, and backup GPTs; recover a wiped partition from
  its backup and scan it.

**Filesystems** (see [docs/plan/04-filesystem-support-matrix.md](docs/plan/04-filesystem-support-matrix.md)):

| Depth | Filesystems |
|---|---|
| Full metadata recovery | APFS (incl. snapshots, checkpoint history, unlocked-encrypted, clones/compression), HFS+ (journaled, case-sensitive), NTFS, exFAT, FAT12/16/32, ext2/3/4, ISO 9660 / Joliet |
| Detect + carve | XFS, Btrfs, F2FS, UFS, ZFS, ReFS, UDF |
| Partitioning | GPT, MBR, APM, hybrid MBR/GPT |
| Containers | raw images, DMG (all UDIF codecs), sparseimage, VMDK, VDI, VHD/VHDX, QCOW2, E01, split sets |

**Not in round 1** (see the [round-2 backlog](docs/build-log/phase-6.md)):
fragment reassembly, RAID, own-crypto unlock (BitLocker/LUKS/FileVault), and
filesystem repair.

---

## Benchmarks

Reclaim publishes its numbers. On every golden image, `content_recall` is **≥
PhotoRec**; named recall is **≥ 0.95** on exFAT/FAT/NTFS/HFS+ delete images and
**≥ 0.90** on APFS (checkpoint-depth reported); peak memory stays under 2 GB
scanning a 512 GiB image. The methodology (doc 09 §3) and the full per-image table
— including the honest overwrite-limit cases — are in
**[docs/benchmarks.md](docs/benchmarks.md)**.

---

## Recovering the Mac's own boot disk

Don't scan a disk you're booted from. Run Reclaim from macOS Recovery instead —
it's a single system-libraries-only binary that runs there unchanged. Full
procedure: **[docs/recovery-mode.md](docs/recovery-mode.md)**.

---

## Forensic soundness

Reclaim produces deterministic result IDs, an NDJSON audit log, and a
`manifest.json` with a BLAKE3/SHA-256 hash for every recovered file. It is **not**
a NIST-CFTT-certified forensic tool and does not claim to be. For chain-of-custody
work, treat it accordingly.

---

## License & contributing

- Licensed under **[Apache-2.0](LICENSE)**. Third-party dependency licenses:
  **[THIRD_PARTY.md](THIRD_PARTY.md)**.
- Signatures are data, not code: contribute a new format with a TOML entry + a
  small synthetic/public-domain sample + a test. See **[CONTRIBUTING.md](CONTRIBUTING.md)**
  (all contributions are DCO sign-off; signature PRs must be clean-room, never
  copied from PhotoRec/TestDisk).
- Security policy and reporting: **[SECURITY.md](SECURITY.md)**.
