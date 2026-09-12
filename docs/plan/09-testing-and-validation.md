# 09 — Testing & Validation Strategy

Recovery software is judged on one number — what fraction of lost files come back intact — and on one property — it never makes things worse. Both must be measured continuously.

## 1. Test pyramid

| Level | What | Tooling |
|-------|------|---------|
| Unit | Parsers on synthetic byte arrays; validators on sample files; extent math | `cargo test`, `proptest` |
| Fuzz | Every FS parser, every validator, image container parsers | `cargo fuzz` (libFuzzer) with seed corpora from real headers; run in CI nightly, 10 min per target; crashes are P0 bugs |
| Golden images | Scripted images with known content and known deletions → expected result set | `testdata/recipes/*.toml` + generator script; images built in CI, never committed |
| Integration | CLI end-to-end on golden images; JSON output diffed against expectations | `insta` snapshots / custom harness |
| Benchmark | Throughput and recovery-rate on the reference corpus | `criterion` + custom scorer; results tracked over time |
| Hardware | Real SD cards, USB HDDs, a T2 Mac, an Apple Silicon Mac, FileVault on/off | Manual checklist per release (§6) |
| Safety | Read-only proof | §5 |

## 2. Golden image generation

Because macOS can create most relevant filesystems natively, generate images on the Mac runner; use Linux runners (Docker) for ext4/XFS/Btrfs/F2FS.

Recipe example (`recipes/exfat-camera-delete.toml`):
```toml
size = "2GiB"
scheme = "mbr"
fs = "exfat"
cluster = "128KiB"
[populate]
corpus = "photos-mixed-200"      # named sample set: JPEG/HEIC/CR2/MP4/MOV mixed sizes
layout = "camera"                # DCIM/100CANON/IMG_%04d.ext
[actions]
steps = [
  { delete = "DCIM/100CANON/IMG_00[0-4]*.JPG" },     # 50 files
  { write = { corpus = "photos-mixed-20", path = "DCIM/100CANON/" } },  # partial overwrite
  { delete = "DCIM/100CANON/MVI_0007.MP4" },
  { fragment = { file = "DCIM/100CANON/MVI_0009.MP4", pieces = 4 } },   # forced fragmentation via interleaved writes
  { detach = true },
]
[expect]
named_recoverable   = "DCIM/100CANON/IMG_00[0-4]*.JPG minus overwritten"
carve_recoverable   = { family = "image", min = 40 }
fragmented_mp4      = ["MVI_0009.MP4"]
```
Generator uses `hdiutil create`, `diskutil eraseVolume`, normal file APIs, `hdiutil detach`; on Linux `mkfs.*` + loop mounts. The generator records ground truth: every file's name, hash, and physical extents (via `fcntl(F_LOG2PHYS)` on macOS, `filefrag`/`FIEMAP` on Linux) *before* deletion, so expectations are exact.

Recipe families to maintain (≈ 40 images, 1–8 GiB each):
- Per FS (APFS, HFS+, NTFS, exFAT, FAT32, FAT16, ext4, ext3, ext2, XFS, Btrfs): deleted files, deleted folders, emptied Trash, partial overwrite, quick format (new FS of same type), re-format to different type, deleted partition, corrupted boot sector / superblock (zeroed first 64 KiB).
- APFS extras: snapshots present, many checkpoints (loop create/delete 500×), cloned files, compressed files, encrypted volume (unlocked) — created via `diskutil apfs`.
- Camera media: exFAT SD images from **real cameras** (record video until card full, delete, record again) — capture with `dd` from the card and keep as private corpus (not in repo; hashes only).
- Damaged media: golden image + bad-block injection layer (`BlockSource` wrapper returning errors at chosen LBAs) to test imager and `Suspect` handling.
- Big: one 512 GiB sparse image with 2 M files for memory/throughput tests (nightly).

## 3. Recovery-rate scoring

For each golden image and engine configuration:

```
named_recall     = named_correct / named_expected        (name + path + hash match)
content_recall   = hash_matches / expected_recoverable   (any engine, ignoring names)
partial_credit   = Σ(longest correct prefix / size) for truncated results
false_positives  = carved results that match no expected file and fail validation
precision        = (hash_matches + valid_unknown) / total_results
time, bytes_read, peak_rss
```
Report as a table per commit; block merges that regress `content_recall` or `named_recall` on any T1 image by > 0.5 pp without a documented reason. Compare against PhotoRec (run in CI on the same images; it's GPL but running it is fine) for the carving column — the goal is ≥ PhotoRec on every image.

## 4. Fuzzing targets (minimum)

`fs-apfs::probe/walk`, `fs-hfs::catalog`, `fs-ntfs::mft_record`, `fs-fat::dir_entry`, `fs-exfat::entry_set`, `fs-ext::inode/dirent/journal`, `reclaim-part::{gpt,mbr,apm}`, `reclaim-block::{dmg_koly,vmdk,e01}`, every validator in `reclaim-carve::validators` (jpeg, isobmff, tiff, zip, ole2, pdf, riff, sqlite, png, mkv, mp3, text). Structure-aware fuzzing with `arbitrary` for header structs where cheap.

## 5. Read-only proof

1. **Static**: `reclaim-block` and every `fs-*` crate has `#![forbid(unsafe_code)]` where possible and a `clippy` lint config; a CI grep denies `OpenOptions::new().write`, `libc::write`, `pwrite`, `ioctl` outside `reclaim-block/src/imaging/dest.rs` and `reclaim-session` (writes to the session DB) — with an allow-list file reviewed on change.
2. **Dynamic**: every integration test hashes the source image before and after; mismatch fails the suite. On macOS hardware tests, mount the source read-only *and* compare `dd`-hashes before/after.
3. **Audit flag**: `reclaim --prove-readonly` prints every `open()` with flags (via a global hook in the platform crate) — tests assert no `O_WRONLY|O_RDWR` on sources.

## 6. Hardware checklist per release

- [ ] Apple Silicon Mac, FileVault on: list, info, scan Data volume in normal boot; scan from Recovery Terminal.
- [ ] Intel T2 Mac: same; plus Target Disk Mode from a host.
- [ ] Intel non-T2 Mac (older): raw boot-disk read.
- [ ] SD cards from 3 cameras (Canon/Sony/GoPro or DJI): format-in-camera then scan; compare with Disk Drill trial & PhotoRec.
- [ ] USB HDD with known bad sectors (keep one!): image tool completes with map; scan the image.
- [ ] NTFS & ext4 externals; Windows-formatted exFAT; FAT32 USB stick.
- [ ] Hot-unplug during scan → resume.
- [ ] Recover 50 GB to external SSD; verify hashes; refuse same-disk destination.
- [ ] Notarization: fresh macOS VM, download DMG, Gatekeeper passes; helper installs; FDA flow works.

## 7. Performance targets & how to measure

- Sequential deep scan: measure MB/s vs `dd bs=4m` baseline on the same device; target ≥ 80 %.
- Metadata engine: APFS Data volume with 2 M files walks in < 60 s from NVMe.
- Memory: `peak_rss` from `/usr/bin/time -l` in the nightly 512 GiB test; cap 2 GB.
- UI: thumbnail grid scroll stays at 60 fps with 100 k results (Instruments).

## 8. Bug triage rules

- P0: any write to a source; any crash on malformed data; data loss in recover.
- P1: regression in recovery-rate; wrong file named with high confidence; refusing a valid destination.
- P2: performance regressions > 20 %; UI hangs.
