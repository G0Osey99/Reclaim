# Phase 0 — Workspace, block layer, enumeration, platform proofs

- **Date:** 2026-09-12
- **Branch:** `dev/v1` (off `main`)
- **Commit range:** the `p0:` commits on `dev/v1` (see `git log main..dev/v1`).
- **Toolchain:** rustc/cargo **1.98.1** (pinned in `rust-toolchain.toml`),
  clippy 0.1.98, rustfmt 1.9.0, cargo-deny 0.20.2, Python 3.14.6.
- **Host (for proofs):** `Mac17,6`, Apple **M5 Max**, macOS **26.6.2** (`25G83`).

## Done

### A. Workspace + CI
- Cargo workspace ([Cargo.toml](../../Cargo.toml)) per doc 03 §4 with exactly the
  three Phase 0 crates: `reclaim-block`, `reclaim-platform-macos`, `reclaim-cli`
  (bin `reclaim`).
- `rust-toolchain.toml` pins stable **1.98.1** + clippy/rustfmt.
- Workspace lints: `clippy::all` warn + CI runs `-D warnings`;
  `reclaim-block` additionally `#![deny(clippy::unwrap_used, expect_used,
  indexing_slicing)]` (the parser-style crate this phase).
- [LICENSE](../../LICENSE) Apache-2.0; [deny.toml](../../deny.toml) with the Part
  3.3 allow-list (MIT/Apache-2.0/BSD-2/BSD-3/ISC/Zlib/Unicode-3.0/MPL-2.0).
- [scripts/check-readonly.sh](../../scripts/check-readonly.sh) + reviewed
  [allow-list](../../scripts/readonly-allowlist.txt) (Part 3.2).
- `cargo ci` entry point via [scripts/cargo-ci](../../scripts/cargo-ci) →
  [scripts/ci.sh](../../scripts/ci.sh): fmt --check · clippy -D warnings · test ·
  deny · check-readonly.
- GitHub Actions [ci.yml](../../.github/workflows/ci.yml): `macos-latest` +
  `ubuntu-latest` on `main`, `dev/**`, PRs.

### B. reclaim-block (doc 03 §2.1)
- `BlockSource` trait exactly as specified (**no write method**), with
  `ReadResult` carrying per-sector good/bad/unread bitmaps and `SourceId`.
- Implementations: `ImageFile` (raw), `RawDevice` (macOS `/dev/rdiskN`, `O_RDONLY`
  via `libc::open`, sector-aligned `pread`, **EIO → Bad sectors, not errors**, with
  per-sector isolation on the slow path), `OffsetView` (partition window),
  `ReadAheadCache` (LRU of 1 MiB chunks, preserves per-chunk bad status),
  `BadBlockMap` (+ JSON (de)serialization), `FaultInjector` (test wrapper marking
  chosen LBAs bad).
- Read-only audit hook `open_log` powering `--prove-readonly`.
- 22 unit tests + 4 property tests (`tests/read_equiv.rs`) covering read
  equivalence across sources, fault injection, and the alignment/sector-count math.

### C. reclaim-platform-macos (doc 06 §1, §7)
- IOKit enumeration of every `IOMedia` with: BSD name, size, logical (preferred)
  block size, whole/leaf, removable/ejectable, content type, and — via the parent
  `IOBlockStorageDevice` Device/Protocol Characteristics — model, serial, bus,
  internal flag.
- Mount point + FS kind via `getmntinfo(3)`; APFS container↔volume relations,
  roles and FileVault state via `diskutil apfs list -plist`; SMART summary via
  `diskutil info -plist` (whole disks). All exposed as a serde-serializable `Disk`.
- Verified live on this Mac (tests enumerate `disk0`, the APFS containers and the
  Data volume).

### D. CLI (`reclaim`)
- `reclaim list` (comfy-table + `--json`), `reclaim info <SOURCE>`
  (identity/geometry/mounts/health + APFS/FileVault/TRIM warnings + a 1 s
  read-speed probe), `reclaim doctor` (root? / FDA via boot-store raw read / SIP
  informational + fix-its). Exit codes per doc 07 §3; `--prove-readonly` logs
  every `open()` with flags and asserts none carried write intent.

### E. Golden-image generator (doc 09 §2)
- [scripts/gen-images](../../scripts/gen-images) (Python 3) + four recipes in
  `testdata/recipes/`. Builds raw images by attaching a `CRawDiskImage`,
  `diskutil partitionDisk`, populating synthetic **non-copyrighted** JPEG/PNG/MP4/
  TXT of varied sizes, deleting per recipe, running 300+ APFS create/delete
  transactions for checkpoint history, detaching. Records ground truth (name,
  size, SHA-256, `F_LOG2PHYS_EXT` extents, deleted flag) to `*.groundtruth.json`.
- All four build locally (no sudo): `exfat-camera-delete`, `fat32-usb-delete`,
  `apfs-delete-history` (churn 320), `hfsplus-delete`. Images gitignored.

### F. Platform proofs (run + diagnosed 2026-09-12)
- Ryker ran [scripts/platform-proofs.sh](../../scripts/platform-proofs.sh); a
  diagnose→adversarially-verify→synthesize workflow (12 agents) plus live probes
  established the real picture, written into
  [doc 06 §12](../plan/06-macos-platform-notes.md) (both `[verify]` markers gone):
  - **Proof 1 — PASS:** a user-attached `hdiutil` image node reads raw (sudo and
    non-sudo; the node is user-owned, not TCC-protected).
  - **Two-gate finding:** internal `/dev/rdisk*` need **DAC** (`root:operator` →
    root or `operator` group) **and** **TCC/FDA**. FDA is already granted+effective
    for the Claude Code helper (`com.anthropic.claude-code`); **`sudo` drops the
    FDA attribution**, so `sudo dd if=/dev/rdisk0` fails `EPERM`. The working path
    is **non-sudo + `operator` group**. `reclaim doctor` now reports both gates and
    classifies DAC vs TCC.
  - **Proof 2 — diagnosed / deferred:** boot-store read fails on DAC (not missing
    FDA). Closeable via operator-group + non-sudo.
  - **Proof 3 — OPEN:** Data volume `disk3s5` (`Encryption=true, FileVault=true`);
    whether the unlocked node returns decrypted APFS vs ciphertext is unproven
    (read was DAC-blocked). NXSB/APSB detector validated on the golden image.
  - **Proof 4 — capability PASS, behavioral retracted:** internal `TRIM Support:
    Yes`; the earlier "reads ZEROS → TRIM happened" was a **false positive** (denied
    read → 0 bytes miscounted). `F_LOG2PHYS` is unusable on the encrypted internal
    volume; a clean erasure-timing test needs an unencrypted external device.

## Decisions made
- **Docs relocated** to `docs/plan/` with `reclaim-build-guide.md` at repo root, to
  match every phase prompt (`docs/plan/…`) and build-guide Part 5.1/Part 6. The
  repo shipped them under `docs/`.
- **`cargo ci` as an external subcommand** (`scripts/cargo-ci`), because a Cargo
  alias can only chain cargo-native subcommands and our gate also runs the
  `check-readonly` shell step. CI (and local, after adding `scripts/` to PATH)
  invoke `cargo ci`; `./scripts/ci.sh` works directly.
- **Toolchain pinned to exact `1.98.1`** (not floating `stable`) so `-D warnings`
  is reproducible — a newer clippy could add lints that break CI.
- **Enumeration data sources:** IOKit is the primary inventory (it is what the
  Phase 5 privileged helper will use); mounts come from `getmntinfo`, APFS
  relations/roles/FileVault from `diskutil apfs list -plist`, SMART from
  `diskutil info -plist`. DiskArbitration hot-plug callbacks are deferred to
  Phase 5 (not needed for a one-shot enumeration). IOKit SMART passthrough
  (IOATASMART/NVMe) is deferred to Phase 4 per doc 06 §7.
- **CoreFoundation FFI** uses the untyped `CFDictionary` + `CFDictionaryGetValue`
  (the only form implementing `ConcreteCFType` for `downcast`).
- **`RawDevice` geometry** via the read-only `DKIOCGET{BLOCKSIZE,BLOCKCOUNT,
  PHYSICALBLOCKSIZE}` ioctls (allow-listed in check-readonly); falls back to
  `fstat` (so it also works on a plain file, keeping Linux CI + unit tests honest).
- **`ReadResult::good(..)`** is the all-good constructor (renamed from `all_good`
  to avoid colliding with the `all_good(&self)` predicate).
- **Physical sector size at enumeration = logical**; the true physical size is
  refined when a source is opened (`reclaim info` reports the ioctl value).
- **exFAT/FAT ground-truth extents are empty** — `F_LOG2PHYS` is unsupported on
  those filesystems; the Phase 2 exFAT/FAT parsers will recover extents from the
  filesystem. APFS/HFS+ extents are captured fully.
- **Synthetic samples** are generated into each image at build time (valid magic +
  framing, random padded bodies); no `testdata/samples/` is committed yet (Phase 1
  adds committed signature samples, Part 3.4).
- No planning-doc factual errors were found this phase beyond the `[verify]`
  resolutions in doc 06.

## Gates
- `cargo ci` **green** locally on macOS: fmt --check, clippy `-D warnings`,
  `cargo test` (**31 tests**: 22 block unit + 4 block property + 1 CLI + 4
  platform), `cargo deny check` (advisories/bans/licenses/sources ok),
  check-readonly (no write/mutate outside the allow-list). CI matrix
  (macos-latest + ubuntu-latest) configured; first push validates the Linux runner.
- `reclaim list` / `info` / `doctor` all run correctly on the host.
- Four golden images build locally with ground-truth sidecars.
- doc 06 has no pending `[verify]` markers.

## Manual items (Ryker) — none block Phase 1
1. **Close proofs 2 & 3 (NO media needed):** `sudo dseditgroup -o edit -a "$USER"
   -t user operator`, then **quit & relaunch Claude Code** (group membership applies
   to new sessions), then run `scripts/platform-proofs.sh` **without sudo**. This
   closes the boot-store read and settles the proof-3 decrypted-vs-ciphertext
   question. Do NOT toggle Full Disk Access — it is already granted and effective.
2. **External TRIM behavioral test (optional):** any external disk with ~100 MB
   free (need not be erasable): `scripts/platform-proofs.sh --ext-dev diskN
   --ext-mount /Volumes/<NAME>`. Sacrificial (erasable) media is only needed for a
   whole-device wipe test, and for Phase 1's manual SD-card spot-check.
3. **CI:** GitHub Actions matrix is green on `dev/v1` (macos-latest + ubuntu-latest,
   run 34705045746) — verified this session.

## Known gaps (deferred, with phase)
- DiskArbitration appear/disappear callbacks → Phase 5 (GUI hot-plug).
- IOKit SMART passthrough (real attributes vs the diskutil summary) → Phase 4.
- Absolute image offsets for ground-truth extents (needs the partition parser) →
  Phase 2; sidecars currently record volume-relative device offsets.
- exFAT/FAT extent capture in ground truth → Phase 2 (from the FS parser).
- Committed `testdata/samples/` signature corpus → Phase 1.
- `--log <file>` is accepted but logs to stderr only until Phase 1.

## Next phase needs (facts so Phase 1 need not rediscover)
- **Host:** `Mac17,6`, Apple M5 Max, macOS 26.6.2, SIP on, FileVault on, internal
  2 TB `APPLE SSD AP2048Z` with TRIM=Yes.
- **Device names:** internal whole disk `disk0` (`/dev/rdisk0`); boot APFS
  container `disk3` (`/dev/rdisk3`, `NXSB` here); **user-data target `disk3s5`**
  (`/dev/rdisk3s5`, FileVault on). No external sacrificial device yet.
- **Golden images:** `testdata/build/{exfat-camera-delete,fat32-usb-delete,
  apfs-delete-history,hfsplus-delete}.img` + `*.groundtruth.json`. Rebuild with
  `scripts/gen-images`. Scan directly with `reclaim info <img>` (no root).
- **Block API:** `reclaim_block::{BlockSource, ReadResult, SectorStatus, SourceId,
  ImageFile, RawDevice, OffsetView, ReadAheadCache, BadBlockMap, FaultInjector,
  open_log}`. `CHUNK_SIZE = 1 MiB`. Reads are sector-aligned; bad sectors are
  values, not errors.
- **Read-only rule:** any new write path must be added to
  `scripts/readonly-allowlist.txt` with a reason (currently only
  `raw_device.rs` for read-only geometry ioctls).
