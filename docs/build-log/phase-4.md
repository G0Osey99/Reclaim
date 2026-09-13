# Phase 4 — Lost structures, ext/ISO engines, image containers, health, Recovery Mode

- **Date:** 2026-09-12
- **Branch:** `dev/v1` (off `main`)
- **Commit range:** the `p4:` commits on `dev/v1`.
- **Toolchain:** rustc/cargo **1.98.1** (pinned); nightly + cargo-fuzz **0.13.2**
  for the libFuzzer targets. New dev tooling installed this phase (Homebrew, no
  sudo): **e2fsprogs** (`mke2fs`/`debugfs`, keg-only at
  `/opt/homebrew/opt/e2fsprogs/sbin`) and **qemu** (`qemu-img`) for the ext and
  container golden images; `hdiutil` for DMG/ISO.
- **Sources for the parsers:** public on-disk-format references only
  (ext2/3/4 layout, ECMA-119/Joliet, Apple DMG `koly`/`mish` cross-checked
  against the permissive `dmg2img`/`libdmg-hfsplus` descriptions, VMDK/VDI/VHD/
  VHDX/QCOW2 vendor docs, the EWF notes behind libewf). **No GPL/LGPL code
  copied** (build guide Part 1.4 rule 2); `cargo deny` stays green.

## Done

### C. Image-container `BlockSource`s — `reclaim-block::container` (doc 04 §5)
Each container opens read-only and presents the **guest raw image**:

| Format | Detection | Notes |
|---|---|---|
| DMG UDIF | `koly` trailer | `mish`/`blkx` block map from the XML plist; chunk types raw/zero/**zlib(UDZO)**/**bzip2(UDBZ)**/**lzfse(ULFO)**/**lzma(ULMO)** |
| Apple sparseimage | `sprs` @0 | band index table (verified against a real `hdiutil` image) |
| VMDK | `KDMV` @0 / descriptor | monolithicSparse grain map; flat/`monolithicFlat` via a descriptor + extents |
| VDI | `0xBEDA107F` @0x40 | block-allocation table |
| VHD | `conectix` footer | fixed (raw) + dynamic (BAT + per-block sector bitmap) |
| VHDX | `vhdxfile` @0 | region table → metadata + interleaved payload/bitmap BAT |
| QCOW2 | `QFI\xfb` @0 | v2/v3 L1/L2 walk, compressed (raw-DEFLATE) clusters |
| EnCase E01 | `EVF\x09…` @0 | EWF section chain, per-chunk zlib; multi-segment `.E0N` |
| split | `*.001` + sibling | concatenation |

One shared `MappedSource` (segment list → backing store + an LRU decode cache)
serves DMG, E01 and every cluster-table format; `ConcatSource` serves split sets
and multi-extent flat VMDK. Decompressors are **pure Rust** (`flate2`
`rust_backend`, `bzip2-rs`, `lzma-rs`, `lzfse_rust`), so no dynamic library is
added — the Recovery build's `otool -L` stays clean. `reclaim_block::
open_image_auto` auto-detects the format and falls back to a raw `ImageFile`; the
CLI opens containers transparently and `info` names the format.

Verified byte-for-byte against **real** `qemu-img` + `hdiutil` output for all 12
format/codec combinations (`crates/reclaim-block/tests/containers_real.rs`);
E01 and sparseimage are validated against synthesized images in unit tests
(no `ewfacquire` on the host).

### A. Lost-structure engine — `reclaim-structs` (doc 03 §2.4, FR-SCAN-4)
A sequential anchor sweep over the source finds every structure in doc 04 §1–§2
and emits `ProposedVolume{start,len,fs,confidence,evidence}`: GPT primary +
**backup** header and its entries, MBR (via `reclaim-part`), APFS `NXSB`/`APSB`,
HFS+ **main + alternate** VH, NTFS boot, exFAT boot (main + sector-12 backup),
FAT BPB, ext `0xEF53`, XFS `XFSB`, Btrfs `_BHRfS_M` @64 KiB, F2FS, UFS1/UFS2,
ISO `CD001`, ZFS uberblock, VMFS. Each validator reads the surrounding
superblock, sanity-checks geometry, and computes the true volume start — a wiped
primary GPT is recovered from the backup, a zeroed exFAT main boot from the
backup boot sector, a zeroed HFS+ main VH from the alternate VH at the volume
end. The session **cross-validates** each proposal by probing the metadata
engines at the offset (`meta::probe_best`).

`reclaim volumes <SOURCE>` lists proposals (stored in `proposed_volumes`);
`volumes <SOURCE> adopt N` yields a `session:<dir>/volume/N` source that `scan`
opens as an `OffsetView` over the original. The structure pass also runs during
`reclaim scan` (new `Event::Volume`). `fs-exfat` gained a backup-boot fallback so
an adopted zeroed-boot volume still scans.

### B. ext2/3/4 + ISO engines, detect-and-carve probes
- **`fs-ext`**: superblock (+ `sparse_super` backup fallback), 32/64-bit group
  descriptors, inode tables; recovers a deleted file's data from its inode
  (ext4 extent tree `0xF30A` or ext2/3 direct/indirect pointers) and its **name**
  from directory `rec_len` slack; jbd2 journal + `0xF30A` unallocated scan as the
  zeroed-map fallback; block allocation bitmap.
- **`fs-iso`**: ISO 9660 + Joliet (UCS-2 names, paths, exact extents); UDF
  detection (AVDP @ sector 256 / `NSR0` VRS) with listing via the hybrid ISO tree.
- **XFS/Btrfs/F2FS/UFS/ZFS/VMFS**: `probe() + block size` only, implemented as
  anchor validators in `reclaim-structs` (detect-and-carve, Part 3.6) rather than
  separate near-empty crates.

### D. Health + robustness (doc 06 §7, FR-DEV-3, doc 07 §4)
- **Health verdict** (`reclaim info`): `HealthVerdict{Good,Warn,Bad,Unknown}`
  combining the IOKit-backed SMART status (via `diskutil` — it already reads the
  IONVMe/IOATA SMART) with the read-error/latency probe (the USB fallback). A
  marginal/failing verdict recommends imaging first; `reclaim scan` warns before
  scanning a SMART-failing whole disk.
- **FR-DEV-3 read-error policy** was already shipped in the Phase-1 imager
  (retry-N → mark bad → continue past; reverse pass; resumable bad-block map).
- **Device-disappeared**: `RawDevice` latches `vanished` on ENXIO/ENODEV/EBADF,
  `BlockSource::vanished()` exposes it, `run_carve` checkpoints and stops when the
  source vanishes mid-scan, and the CLI exits **4** (resumable) with a
  reconnect-and-resume message.
- **Resume re-identification** (doc 07 §4): `source.json` records blake3 of the
  first and last MiB; `scan --resume` refuses a session whose source content no
  longer matches (alongside the `SourceId` check).

### E. Recovery-Mode build (doc 06 §4, doc 10 M5)
The CLI already has no GUI frameworks and links only system libraries.
`scripts/build-recovery.sh` builds the universal (arm64 + x86_64) release and
lipo's it; `scripts/check-recovery-libs.sh` asserts `otool -L` shows only
`/usr/lib/**` + `/System/Library/**` (wired into `ci.sh` on macOS);
`scripts/recovery-usb.sh` stages the binary onto a USB volume (copy-only, refuses
the boot volume); `docs/recovery-mode.md` is the exact Recovery Terminal
procedure (SIP/`csrutil` untouched).

## Decisions made
- **One `MappedSource` for all block-mapped containers.** DMG/E01 (variable
  chunk lists) and the cluster-table formats coalesce into one sorted segment
  list (zero/raw/compressed) with a shared, LRU-cached decode path — the fiddly,
  fuzzed logic lives in one place.
- **Pure-Rust decompressors only** (`flate2 rust_backend`, `bzip2-rs`,
  `lzma-rs`, `lzfse_rust`). They add no dylib, so the Recovery build stays
  system-libs-only. These are the only new dependencies (all MIT/Apache/Zlib).
- **Doc 04 §2 (ext) correction.** A typical/`debugfs` delete leaves the ext4
  extent tree **and** the ext2 block pointers intact in the inode, so the inode
  itself is the primary recovery path; the journal + `0xF30A` scan are the
  fallbacks for a truly zeroed map (the doc implied ext3/4 always zero it).
- **Detect-and-carve FSs live in `reclaim-structs`**, not separate crates — their
  whole deliverable is a magic + block size, which the anchor sweep already needs.
- **HFS+/exFAT backup disambiguation.** When a superblock is found and its partner
  (alternate VH / main VBR) is present, the placement is confirmed outright;
  when the partner was zeroed, both plausible volume starts are proposed and the
  scan/cross-validation resolves which is real.
- **`check-readonly` scopes to production code** (skips `tests/`, `benches/`,
  `fuzz_targets/`) — test scaffolding writes only to temp dirs, never a source.

## Gates
- `cargo ci` **green** on macOS: fmt; clippy `--workspace --all-targets
  -D warnings`; `cargo test --workspace` (adds the container round-trip, ext/iso
  recovery, lost-structure proposal, and `fuzz_smoke` suites); `cargo deny` (only
  the four permissive compression crates added); `check-readonly`; and the new
  macOS-only `check-recovery-libs` (otool — system libs only).
- **Golden images** (built by `scripts/gen-p4-golden.sh` + `scripts/gen-fs-images`;
  verified via the CLI):

  | image | how | result |
  |---|---|---|
  | ext4-delete | mke2fs + debugfs rm | 3/3 deleted recovered by name + exact content |
  | ext2-delete | mke2fs + debugfs rm | 3/3 deleted recovered by name + exact content |
  | ntfs-in-vmdk | qemu-img → VMDK | 8 deleted recovered through the container |
  | apfs-in-dmg | hdiutil → UDZO DMG | 1000 deleted recovered through the zlib DMG |
  | gpt-deleted-partition | primary GPT wiped | partition recovered from the backup GPT |
  | exfat-zeroed-boot | main VBR zeroed | volume found at the true start via backup boot; 20 deleted recovered |
  | hfs-zeroed-vh | main VH zeroed | volume found via the alternate VH; 7 deleted recovered |

- **Fuzzing:** four new libFuzzer targets (`ext`, `iso`, `container`, `structs`)
  each ran **130 s clean — 0 crashes** (ext 5.3M execs / cov 154, iso 28.5M /
  cov 81, container 974k / cov 618, structs 2.85M / cov 566). `scripts/fuzz-fs.sh`
  re-runs them (`FUZZ_TARGETS="ext iso container structs"`). CI-runnable stand-in:
  the in-tree `fuzz_smoke` tests for fs-ext, fs-iso and reclaim-structs.
- **Recovery build:** universal (arm64 + x86_64) binary `otool -L` links only
  IOKit, CoreFoundation, libiconv and libSystem — all present in a Recovery
  Terminal.

## Health heuristics chosen (doc 06 §7)
`assess_health(smart, read_errors, bytes_per_sec, slow_threshold)`:
- SMART `Failing` **or** any probe read error ⇒ **Bad** (image first);
- a successful probe slower than **5 MB/s** ⇒ **Warn**;
- SMART `Verified`/absent + a clean, fast probe ⇒ **Good**;
- otherwise **Unknown**.
SMART comes from `diskutil info`'s `SMARTStatus`, which surfaces the kernel's
IOKit SMART; a full `IONVMeSMARTInterface` user-client FFI (per-attribute
reallocated/pending counts) is deferred (see gaps).

## Recovery-Mode procedure (summary; full text in docs/recovery-mode.md)
1. `scripts/build-recovery.sh` → universal binary (system-libs-only, verified).
2. `scripts/recovery-usb.sh <bin> /Volumes/<USB>` copies it onto a USB stick.
3. Boot to Recovery (Apple silicon: hold power → Options; Intel: ⌘‑R), open
   **Utilities → Terminal**. `csrutil` is never touched.
4. `/Volumes/<USB>/reclaim-recovery/reclaim list` / `info diskN` / `scan diskNsM
   --session /Volumes/<USB>/session` → `recover … /Volumes/<USB>/Recovered`.
   Terminal runs as root in Recovery, so no `sudo`/FDA dance; recover to the USB
   (same-disk destinations are refused). FileVault volumes: unlock in Disk
   Utility first. A dying drive: `reclaim info` verdict → image first.

## Manual items (Ryker) — none block Phase 5
1. **External sacrificial media proof:** run `reclaim info diskN` on a real USB
   stick / failing HDD to see a `marginal`/`FAILING` verdict from the read probe
   (the host's internal SSD probes `good`). Hot-unplug mid-`scan` to see the
   device-disappeared exit 4 + `--resume`.
2. **Recovery Terminal run:** boot Recovery, copy the universal binary via
   `scripts/recovery-usb.sh`, and confirm `reclaim list`/`scan` run there.
3. **Re-run the CI matrix** (macOS + ubuntu) on the pushed `p4:` commits; ubuntu
   has native `mke2fs`/`debugfs` + `qemu-img` for the ext/container goldens.

## Known gaps (deferred, with phase)
- **VMDK streamOptimized** (compressed grains), **E01 real-EnCase corpus**
  (synthesized-image validated only; no `ewfacquire` on host), **multi-node
  sparseimage band tables** (> ~1 GiB) — recognized + documented; revisit if a
  corpus needs them.
- **UDF full file-entry walk** — detected + listed via the hybrid ISO tree; a
  native UDF directory walk is a later item.
- **XFS/Btrfs/F2FS/UFS/ZFS** are detect-and-carve only (Part 3.6); no metadata
  walk (T2/T3).
- **IOKit SMART attribute FFI** (reallocated/pending counts, NVMe critical
  warning) — the verdict uses the diskutil SMART status + read probe instead; a
  deeper `IONVMeSMARTInterface` user-client is a Phase-6 item.
- **Structure pass shares the scan's read pattern, not its buffer** — it does its
  own bounded sequential sweep; folding it into the carver's chunk loop is a
  Phase-6 streaming optimization.
- **`--force-source`** is surfaced in the mismatch message but not yet a CLI flag.

## Next phase needs (facts Phase 5 should not rediscover)
- **New crates:** `reclaim-structs` (`scan(src) -> Vec<ProposedVolume>`;
  `FsAnchor` table in `anchors.rs`, GPT in `gpt.rs`), `fs-ext`, `fs-iso` — the
  last two implement `FileSystem` and are wired into `reclaim-session::meta::
  engines()` (now `[EngineDef; 7]`). `meta::probe_best` cross-validates an offset.
- **Containers:** `reclaim_block::open_image_auto(path)` / `detect_container` do
  format detection; container sources are `MappedSource`/`ConcatSource`.
- **Adopted volumes:** source spec `session:<dir>/volume/<n>` → `Resolved::Volume`
  → `OffsetView`; proposals persist in `proposed_volumes` (`Store::{replace,list}
  _proposals`, `ProposalRow`).
- **Health:** `reclaim_platform_macos::{HealthVerdict, assess_health}`; `info`
  prints it, `scan` warns on Failing.
- **Resume:** `SourceInfo.{first,last}_mib_hash` (blake3) in `source.json`;
  `reclaim_session::source_hashes(src)`. `ScanReport.vanished` + `BlockSource::
  vanished()` drive the device-disappeared exit 4.
- **Recovery build:** `scripts/build-recovery.sh` + `check-recovery-libs.sh`
  (in `ci.sh` on macOS); `docs/recovery-mode.md`.
- **Golden images:** `scripts/gen-p4-golden.sh` rebuilds the Phase-4 set from the
  existing raws + e2fsprogs/qemu-img/hdiutil.
