# 10 — Roadmap

Sized for one focused developer with occasional help; multiply out if part-time. Milestones are cumulative and each ends with something usable. Requirement IDs refer to doc 02.

## M0 — Foundations & platform proofs (2–3 weeks)
Goal: answer the [verify] questions in doc 06 and lay the workspace.
- Cargo workspace, CI (macOS + Linux runners), lint config, `#![deny(clippy::unwrap_used)]` in parser crates.
- `reclaim-block`: `BlockSource`, `RawDevice`, `ImageFile` (raw only), `OffsetView`, read cache, bad-block map type.
- `reclaim-platform-macos`: IOKit + DiskArbitration enumeration; open `/dev/rdiskN`; `reclaim list` works under `sudo`.
- Experiments (write results into doc 06): FDA vs raw boot-disk read on macOS 14/15/26; reading unlocked FileVault volume via synthesized device; Recovery Mode Terminal run of a static CLI; TRIM-after-delete timing on internal SSD vs external SSD vs SD.
- Golden-image generator skeleton (exFAT + FAT32 + APFS + HFS+ recipes).
- Deliverable: `reclaim list`, `reclaim info`, `reclaim doctor`.

## M1 — Imager + carving engine (3–4 weeks)
- Multi-pass imager with map, resume, sparse, hashes (FR-IMG-1..5).
- Carving core: TOML catalog → Aho-Corasick; strategies; block-size inference; unallocated-only when bitmap known (later); session SQLite; NDJSON events; checkpoint/resume.
- Validators: JPEG, PNG, GIF, TIFF(+RAW refinement), ISOBMFF, RIFF, ZIP(+OOXML/iWork refinement), PDF, OLE2, SQLite, text, gzip/xz/bz2/7z/rar, Mach-O/ELF/PE, DMG koly, bplist, MP3/FLAC, MKV, MPEG-TS.
- `reclaim scan --deep`, `results`, `recover` (with same-device refusal), `report json`.
- Benchmark vs PhotoRec on 5 golden images; must be ≥ PhotoRec on content_recall.
- Deliverable: a PhotoRec-class CLI carver with previews-by-extraction and sessions.

## M2 — Metadata engines, Tier 1 filesystems (5–7 weeks)
- `reclaim-part`: GPT (with backup), MBR/EBR, APM; APFS container discovery; Core Storage pass-through detection.
- `fs-exfat`, `fs-fat` (deleted entries, LFN, contiguity heuristic), `fs-ntfs` (MFT walk, runs, $I30 slack, USN), `fs-hfs` (catalog, node slack, journal), `fs-apfs` (checkpoints, omap, FS tree, snapshots, orphan nodes — the big one).
- Allocation bitmaps feed the carver (unallocated-only mode; overwritten-extent scoring).
- Merge/de-dup of carved vs named; recoverability score.
- `reclaim scan --quick`, tree output, `snapshots` command.
- Golden images for every T1 FS + APFS extras; named_recall ≥ 95 %.
- Deliverable: full CLI recovery for Mac/Windows/camera media with names and paths.

## M3 — GUI v1 (5–6 weeks)
- UniFFI bindings; privileged helper via `SMAppService`; onboarding & FDA flow.
- Screens: sources, source detail/scan plan, live results (grid/list/tree), inspector with previews (image/HEIC/RAW-embedded-JPEG/PDF/text/hex; video first frame via AVFoundation on the *recovered temp file* or a small in-core MP4 frame extractor), recover sheet, image tool, reports HTML.
- Signing, notarization, Sparkle, DMG packaging, Homebrew cask/formula.
- Deliverable: **Reclaim 1.0** (public beta).

## M4 — Lost structures, ext family, containers (4–5 weeks)
- `reclaim-structs`: anchors for all FSs in doc 04; proposals; adopt flow (CLI + GUI).
- `fs-ext` (ext2 pointer recovery; ext3/4 journal-based; dirent slack), ISO9660/UDF listing.
- Image containers: DMG UDIF (zlib/bzip2/lzfse/lzma), VMDK, VDI, VHD/VHDX, QCOW2, E01, split.
- LVM2, mdadm superblock parsing (linear/mirror members exposed as sources).
- Deliverable: 1.1.

## M5 — Recovery Mode & health (2–3 weeks)
- Static-ish CLI build verified from Recovery Terminal; documented procedure; script to build a bootable external installer with Reclaim.
- SMART via IOKit for internal drives; probe-based health for USB; policy that nudges to Image-first.
- Auto-pause on device disappearance; resume on reappear.
- Deliverable: 1.2.

## M6 — Fragment reassembly & video (6–8 weeks)
- JPEG bifragment/multi-fragment reassembly with decoder-guided splicing.
- MP4/MOV: `moov`-driven chunk validation, missing-chunk search, `moov`-less reconstruction from `mdat` (H.264/HEVC NAL parsing), orphan `moov`↔`mdat` matching.
- Camera-specific brands (GoPro `GPMF`, DJI, Insta360 trailers, Canon CR3/CRM, Sony XAVC MXF).
- Deliverable: 1.3 — the marketing feature.

## M7 — RAID, XFS/Btrfs, encryption (6–8 weeks)
- `reclaim-raid`: virtual RAID 0/1/5/6/10; parameter inference from FS structures; GUI builder.
- `fs-xfs`, `fs-btrfs` (backup roots, tree-node scanning, snapshots).
- `reclaim-crypto`: BitLocker (password/recovery key), LUKS1/2; encrypted DMG.
- Deliverable: 2.0.

## Later / opportunistic
- Deletion-journal background agent ("vault") using FSEvents + `unlink` interception is impossible without kexts; realistic version: FSEvents + periodic snapshotting of directory metadata → later recovery has names. Evaluate value.
- Own FileVault/APFS-encryption unlock from images.
- ZFS, UFS2, F2FS, VMFS listing.
- Windows/Linux GUI (Tauri or native).
- Forensic mode: E01 output, audit log, chain-of-custody report, hash sets.

## Suggested order of first 10 working days
1. Workspace + CI + `BlockSource` + raw device open + `list`.
2. Experiments on FDA/T2/FileVault/TRIM (doc 06) — write findings down.
3. Golden generator for exFAT + FAT32 (fastest to build, most valuable persona).
4. Carver skeleton with JPEG + ISOBMFF + TIFF validators.
5. `scan --deep` + `results` + `recover` on an SD card image; compare to PhotoRec.
