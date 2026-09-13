# Changelog

All notable changes to Reclaim are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] — 2026-09-12

First stable release: the round-1 CLI core, the macOS app, published benchmarks,
and the signed/notarized distribution. Summarizes phases 0–6 (pre-1.0 tags
`v0.1.0` and `v0.5.0-beta` were milestones on the way here).

### Added
- **Read-only block layer & device enumeration** — `O_RDONLY` raw-device access
  with a compile-time no-write guarantee to engines; layered `BlockSource` stack;
  `reclaim list` / `info` / `doctor`.
- **Signature carving** — 140 signatures across 210 file extensions (images,
  22 camera-RAW formats, video, audio, documents, archives, databases, …) with
  38 format-aware validators to reject false positives; contiguous carving.
- **Imager** — byte-for-byte imaging with a bad-sector map, multi-pass
  retry/reverse read, resume, sparse and zstd output, and `verify-image`.
- **Sessions** — one SQLite file per session, deterministic result IDs,
  checkpoint every 5 s / 1 GiB, NDJSON event log, pause/resume, on-disk result
  index (bounded memory).
- **Metadata recovery** — APFS (snapshots, checkpoint history, orphans, clones,
  compressed, unlocked-encrypted), HFS+ (journaled & case-sensitive), NTFS,
  exFAT, FAT12/16/32, ext2/3/4, ISO 9660 / Joliet — live and deleted entries with
  names, paths, timestamps, extents, and a recoverability score.
- **Lost-structure search** — `reclaim volumes` finds boot sectors, superblocks,
  APFS checkpoints/volume superblocks and backup GPTs, recovers a wiped partition
  from its backup, and adopts a proposal as a scannable source.
- **Containers** — DMG (all UDIF codecs), sparseimage, VMDK, VDI, VHD/VHDX,
  QCOW2, E01, and split sets, auto-detected.
- **Detect + carve** for XFS, Btrfs, F2FS, UFS, ZFS, ReFS, UDF.
- **Results & recovery** — filter/sort/search; preview (image/text/hex, embedded
  JPEG for RAW/HEIC); recover with path preservation, collision policy,
  verify-after-copy, and a `manifest.json`; JSON/HTML/CSV reports.
- **Same-disk refusal** — recovery onto the source is refused unless the explicit
  `--allow-same-device-i-accept-data-loss` override is given.
- **Drive health** — SMART-backed verdict that recommends imaging first; device-
  disappeared handling with resumable sessions.
- **Recovery-Mode build** — a system-libraries-only universal CLI that runs from
  the macOS Recovery Terminal (`docs/recovery-mode.md`).
- **Reclaim.app** — SwiftUI app over the Rust core via UniFFI: onboarding
  (read-only-privileged helper via `SMAppService` + Full Disk Access), live
  streaming results with thumbnails during a scan, filter rail, inspector with
  previews, recover sheet with same-disk refusal and verify, imaging tool, lost-
  volume adoption, and the APFS snapshot browser. VoiceOver-navigable, no custom
  chrome.
- **`--prove-readonly`** — audit mode that logs every device `open()` with flags.
- **Published benchmarks** — `docs/benchmarks.md`: methodology and a per-image
  table vs PhotoRec across every golden image, plus the 512 GiB / 2 M-file memory
  and throughput test.
- **Docs** — generated CLI reference (`docs/cli.md`), man pages and shell
  completions, `CONTRIBUTING.md` (DCO + clean-room signature rules), `SECURITY.md`,
  `THIRD_PARTY.md` (via `cargo about`), and the first-run/license notice
  (`docs/EULA.md`).

### Fixed (Phase-6 QA)
- **FAT/exFAT single-cluster validity** — a deleted file that fits in a single
  cluster is now reported `Full`, not `Suspect`; contiguity is a certainty, not an
  assumption, for a one-cluster file (previously over-conservative on large-cluster
  camera cards).
- **Carved↔named merge** — a carved result that begins at the same offset as a
  metadata-recovered file now collapses into it even when the carve over-runs the
  file's true end, removing truncated duplicate results (FR-SCAN-6).
- **Benchmark precision metric** — the scorer now computes precision per
  docs/plan/09 §3 (matches + valid_unknown / total) from the recover manifest,
  instead of a matches-only ratio that penalized correctly-recovered real files.

### Safety & licensing
- Apache-2.0 core; no GPL/LGPL code vendored (`cargo deny` + `deny.toml`).
- No telemetry in the core or CLI.

### Known limitations (round-2 backlog)
- No fragment reassembly (contiguous carving only), no RAID assembly, no
  own-crypto unlock (BitLocker/LUKS/FileVault), no filesystem repair, no
  Windows/Linux GUI. Deeper ext/XFS/Btrfs metadata is future work. See
  `docs/build-log/phase-6.md`.

[1.0.0]: https://github.com/G0Osey99/reclaim/releases/tag/v1.0.0
