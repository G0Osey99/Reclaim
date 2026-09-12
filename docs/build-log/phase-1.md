# Phase 1 — Carving engine, imager, sessions, CLI

- **Date:** 2026-09-12
- **Branch:** `dev/v1` (off `main`)
- **Commit range:** the `p1:` commits on `dev/v1` (`git log b9d4778..HEAD`).
- **Toolchain:** rustc/cargo **1.98.1** (pinned); nightly **1.100** + `cargo-fuzz`
  0.13.2 for fuzzing only; PhotoRec **7.2** (`brew install testdisk`) as the
  benchmark competitor.
- **Tag:** `v0.1.0`.

## Done

### A. reclaim-sigs — signature catalog (data + codegen)
- `crates/reclaim-sigs/catalog/*.toml` per doc 05 §1 (11 family files). `build.rs`
  validates every entry (unique ids, well-formed hex/wildcard patterns, a
  non-empty literal anchor per header, valid strategy, footer present when
  required) and codegens a static `SIGNATURES` table plus `MAX_ANCHOR_LEN` /
  `MAX_HEADER_SPAN` (`crates/reclaim-sigs/src/lib.rs`).
- **140 signatures** seeded (Tier-1 fully, plus Tier-2/3 breadth). Camera RAW is
  complete (22 formats). `reclaim sigs list|test <file>`.

### B. reclaim-carve — carving engine (doc 03 §2.3, doc 05)
- Aho-Corasick matcher over each signature's anchor (longest literal run),
  masked-pattern confirm, block-aligned fast path; single-byte anchors excluded.
- Sequential reader + a statistical **text pass** at inferred block boundaries
  (headerless text has no anchor).
- **32 validators** (doc 05 §4): jpeg (marker walk), png (chunk+CRC-32), gif,
  bmp, tiff (IFD walk + RAW `Make` refinement, EXIF date/model/thumb), isobmff
  (box walk; mdat-first, largesize, size-0; brand refinement), riff, iff, zip
  (+ooxml/odf/iwork/epub/3mf refinement, EOCD sizing), ole2, pdf (last `%%EOF`),
  sqlite (exact size), text (printable/UTF-8 + IoC), gzip/bzip2/xz/zstd/7z/rar/
  tar, mach-o/elf/pe, dmg (koly footer-anchored reverse), bplist, mp3/flac/ogg,
  mkv/ebml, mpeg-ts, iso9660, wasm, ico.
- Extractor strategies fixed/header_size/footer/structural/statistical;
  block-size inference via GCD of validated header offsets; dedup/priority
  (a Full result claims its byte range; inner matches suppressed) per doc 05 §2;
  metadata → synthesized recovery path (doc 05 §6). **Contiguous carve only**;
  fragmented media → `Truncated` (reassembly is round 2).

### C. Imager (doc 02 FR-IMG) — `reclaim-block/src/imaging/`
- Chunk-oriented multi-pass: large read → per-sector retry (pass 2) → optional
  reverse retry (pass 3); raw+sparse or **zstd-framed** output with a
  random-access frame index; per-chunk + whole-image BLAKE3/SHA-256;
  `.reclaim-map` sidecar with bad/unread ranges + first/last-MiB identity;
  `--resume`; `MappedImage` BlockSource (`Unread` for never-imaged ranges);
  `verify-image`. `dest.rs` is the only new write path (allow-listed).

### D. reclaim-session (doc 03 §2.5, Part 3.5)
- One session directory (`session.sqlite`/`source.json`/`plan.json`/
  `log.ndjson`/`thumbs/`); SQLite schema (sources, scan_runs, entries, carved,
  fragments, proposed_volumes, progress, events). Deterministic ids
  `blake3(source||engine||offset||len)`. Bounded-channel single SQLite writer,
  checkpoint every 5 s / 1 GiB, SIGINT → checkpoint → exit 4 (2nd SIGINT →
  immediate). `--resume` from the stored cursor with a **fixed** block size →
  identical result set (deterministic ids). NDJSON events per doc 07 §2. Planner
  chooses engines (carve only) + block size.

### E. reclaim-report + CLI
- `reclaim-report`: JSON/HTML/CSV (FR-RES-5).
- CLI: `scan --deep`, `results` (filters, `--format table|json|csv|ids`),
  `recover` (`--ids/--all`/filters, preserve-paths/flat, collision, `--verify`,
  `manifest.json`, same-whole-disk refusal → exit 5), `report`, `preview`
  (raw bytes / embedded thumbnail), `image`, `verify-image`, `sigs list|test`.

### F. Benchmark (doc 09 §3)
- `scripts/bench.sh` + `bench_score.py` run reclaim and PhotoRec on the four
  Phase-0 images and score exact SHA-256 content recall against each
  ground-truth sidecar. `docs/build-log/phase-1/bench.md`.

  | image | reclaim recall | photorec recall |
  |---|---|---|
  | apfs-delete-history | 82.5% | 70.0% |
  | exfat-camera-delete | 100.0% | 58.3% |
  | fat32-usb-delete | 85.0% | 50.0% |
  | hfsplus-delete | 100.0% | 85.0% |

  **Target met on every image** (reclaim ≥ PhotoRec). Precision 85–100%; deep
  scan 1–2.4 s per image; peak RSS ~130 MB. reclaim's JPEG recall is dramatically
  better than PhotoRec's on these synthetic images (PhotoRec's JPEG carver
  rejects/truncates the structurally-valid-but-non-decodable synthetic JPEGs).

## Decisions made
- **One anchor per shared container magic; validators refine.** RIFF→wav/avi/
  webp, ZIP→ooxml/odf/iwork/epub/3mf, ISOBMFF ftyp→mp4/mov/heic/cr3/m4a/braw,
  TIFF→~15 RAWs by `Make`. The engine groups candidates by validator per offset
  and attributes the result to the validator's refined id.
- **Single-byte anchors excluded from the matcher.** Only MPEG-TS's `0x47`; it
  needs a periodic-sync pass (a documented gap), so the `mpegts` validator exists
  and is fuzzed but is not reached by the AC scan.
- **Block-size inference uses the GCD of validated header offsets** and is
  computed once by the planner, stored in `plan.json`, and reused on `--resume`
  so the block-alignment gate — and therefore the result set — is deterministic.
- **The statistical text pass probes at the inferred block size** (the offset
  GCD), not a coarser fixed stride: exFAT/FAT cluster heaps are not 4 KiB-aligned
  in absolute terms, so a coarser stride missed text starts and shifted content
  (found via the golden-image benchmark; fixing it took exFAT text recall 0→100%).
- **Only a validator can accept.** Header-only formats with no validator fall
  back to a footer scan or a small bounded `Suspect` carve, so they never claim
  a range or produce giant results.
- **`ico`/`gzip` hardened for precision.** A permissive ico produced a bogus
  2.2 MB "Full" result that claimed a range and suppressed real JPEGs/MP4s; ico
  now requires every directory entry to be plausible. gzip rejects random
  `1F 8B 08` via FLG-reserved-bit and OS-byte checks.
- **zstd added for the imager** (`zstd` crate; the C library is BSD-3, chosen by
  cargo-deny). Exact archive sizing via decompression is out of scope this phase
  (gzip/bzip2/xz/zstd/rar/bplist/ole2/flac carve as bounded `Suspect`).
- **Same-whole-disk refusal**: image sources compare by filesystem device,
  raw-device sources by whole-disk BSD name (macOS `statfs`); override with
  `--allow-same-device-i-accept-data-loss`.
- No planning-doc factual errors beyond the doc 05 magic-byte tightenings noted
  in the catalog (`raw.rw2` trimmed to the 4-byte discriminator; JPEG uses one
  `FF D8 FF ??` anchor covering every first-marker variant).

## Gates
- `cargo ci` **green** on macOS: fmt --check, clippy `-D warnings`,
  `cargo test --workspace` (**≈150 tests** incl. the scan/resume gate and the
  exFAT golden-image smoke test), `cargo deny` (licenses/advisories ok, zstd C
  lib resolves BSD-3), `check-readonly` (writes only in the imager dest, session,
  report, and CLI recover/preview — all allow-listed).
- **Fuzzing:** a `cargo fuzz` target per validator (33). `scripts/fuzz-all.sh`
  builds all and runs each **≥ 120 s** (nightly + `cargo-fuzz`). Run this
  session: **all 33 targets passed clean, 0 crashes.** The first run surfaced a
  `tiff` slow-unit (unbounded SubIFD recursion / huge value arrays) — fixed with
  work budgets and re-fuzzed clean; the reproducer is kept as a corpus seed.
- **Scan/resume:** interrupt at 40% → resume → identical result set
  (`crates/reclaim-session/tests/resume.rs`).
- **recover** refuses a same-disk destination (exit 5) unless the override flag
  is given (verified on the golden images; live-device refusal is Ryker's manual
  check).
- **Benchmark:** table written; `content_recall ≥ PhotoRec` on every image.

## Manual items (Ryker) — none block Phase 2
1. **SD-card spot check (doc 07 §Your-check):** delete a few photos from the
   sacrificial SD card, then `sudo reclaim scan --deep disk4 --session ~/s1`,
   `reclaim results ~/s1 --family image`, `reclaim recover ~/s1 ~/Recovered
   --verify`, open a recovered JPEG. Confirm `recover` refuses the card itself as
   a destination.
2. **CI Linux runner:** the GitHub Actions matrix (macos + ubuntu) should be
   re-run on the pushed `p1:` commits; the fuzz job is local-only (nightly).

## Known gaps (deferred, with phase)
- **Fragment reassembly** (JPEG bifragment, MP4/MOV) — round 2 (doc 05 §5);
  fragmented media is emitted `Truncated`.
- **Exact sizing for compressed archives** (gzip/bzip2/xz/zstd/rar) and forward
  sizing for bplist/ole2 — needs decompression / trailer handling; carved as
  bounded `Suspect` for now.
- **MPEG-TS** needs a periodic-sync scan pass (single-byte anchor excluded).
- **Text exact recall varies by image** (apfs/fat32 lower than exfat/hfsplus):
  statistical text sizing depends on cluster slack being zeroed and on the block
  inference; still ≥ PhotoRec on every image.
- **`--unallocated-only`, `--threads`, `--skip`, `--passes`** are accepted but
  are no-ops/aliases until Phase 2 (FS allocation bitmap) / profiling.
- **Streaming/bounded memory for multi-TB sources**: results are collected per
  scan; fine for the golden images (peak RSS ~130 MB), revisit for the nightly
  512 GiB test (Phase 6).
- Partial-credit scoring needs original file bytes (sidecar stores only hashes).

## Next phase needs (facts so Phase 2 need not rediscover)
- **Crates:** `reclaim-sigs` (catalog + `by_id`/`by_family`/`all`),
  `reclaim-carve` (`CarveEngine`, `CarveOptions`, `validators::*`, `reader`,
  `Metadata`, `Validity`), `reclaim-session` (`Session`, `Store`, `QueryFilter`,
  `ScanConfig`, `Event`), `reclaim-report`, `reclaim-block::imaging::*`.
- **Carver → merge hook (Phase 2 §2.3 step 6):** carved results carry
  `offset`/`len`/`block_aligned`; a metadata-recovered entry whose extents equal
  a carved range should collapse into the named entry. The carver already labels
  `block_aligned`; add allocation-bitmap consultation (`--unallocated-only`) when
  the FS engine lands.
- **Session schema** already has empty `entries`/`fragments`/`proposed_volumes`
  tables for Phases 2/4.
- **Block API unchanged** from Phase 0; imaging adds `MappedImage` and the
  `.reclaim-map` sidecar for damaged-media scans (Phase 4).
- Golden images + `bench.sh` in place; extend with `named_recall` in Phase 2.
