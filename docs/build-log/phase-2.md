# Phase 2 — Partition tables + exFAT / FAT / NTFS metadata recovery

- **Date:** 2026-09-12
- **Branch:** `dev/v1` (off `main`)
- **Commit range:** the `p2:` commits on `dev/v1` (`git log 4d2a485..HEAD`).
- **Toolchain:** rustc/cargo **1.98.1** (pinned). cargo-fuzz is **not installed**
  on this host (see Gates); the in-tree randomized `fuzz_smoke` tests are the
  CI-runnable robustness proof, and `crates/reclaim-fs-fuzz` holds the libFuzzer
  targets for a nightly runner.
- **Benchmark competitor:** PhotoRec **7.2** (`brew install testdisk`).

## Done

### A. reclaim-part — partition schemes (docs/plan/04 §1)
- `crates/reclaim-part`: `scan(&BlockSource) -> PartitionMap{scheme, entries,
  confidence, notes, containers}`.
  - **GPT** (`gpt.rs`): primary header at LBA 1, **CRC-32 validated** (header +
    entry array); falls back to the **backup header** at the last LBA when the
    primary CRC fails. Apple type GUIDs recognised (APFS `7C3457EF…`, HFS+
    `48465300…`, Core Storage `53746F72…`, Boot `426F6F74…`) plus the common
    EFI/MS/Linux ones; GPT names decoded (UTF-16LE); GUID mixed-endian decode.
  - **MBR/EBR** (`mbr.rs`): the four primary entries plus **EBR extended
    chains** (`0x05/0x0F/0x85`), non-advancing-chain guard.
  - **Hybrid MBR**: a GPT disk whose MBR carries non-`0xEE` entries is reported
    `hybrid-mbr` with both the GPT and the extra MBR partitions.
  - **APM** (`apm.rs`): detect-only (`ER` at block 0, `PM` at block 1).
  - **APFS container / Core Storage**: detect-only (`NXSB` at block 0+32; CS
    signature; GPT type GUIDs) — consumed in Phases 3/4.
  - CRC-32 self-implemented (`crc.rs`, ISO-HDLC), verified against known vectors.
  - `reclaim info` prints the map + containers; `--json` adds `partition_map`.

### B. reclaim-fs-core — the engine contract (docs/plan/03 §2.2)
- `crates/reclaim-fs-core`: the `FileSystem` trait (object-safe: `kind`,
  `block_size`, `allocation_bitmap`, `walk`, `read_extents`, `snapshots`) and the
  `Entry` type exactly per §2.2 — `state: Live|Deleted|Orphaned|Historical`,
  `confidence`, **raw name bytes + UTF-8 name**, `extents`, `contiguous_assumed`.
- `EntrySink` (the session's sink shifts extents volume→absolute and stores),
  `Bitmap` (cluster allocation mapped back to source bytes), `WalkOpts`,
  `Probe`, and `score::recoverability` = structural confidence × unallocated
  fraction × not-overwritten (chain-assumed files penalised), 0..100 to match the
  carver's score.

### C. The three metadata engines
- **fs-exfat** (docs/plan/04 §3.4, from the Microsoft exFAT spec): boot sector +
  **boot checksum**, up-case presence, `$BITMAP`, directory walk incl. **deleted
  entry sets** (0x05/0x40/0x41 keep name/size/first-cluster), `NoFatChain` ⇒
  contiguous, zeroed FAT chain ⇒ assume contiguous + `Suspect`. Names from the
  File-Name entries (UTF-16).
- **fs-fat** (FAT12/16/32, from the Microsoft FAT spec): BPB + **backup boot
  sector** fallback, FAT-type by cluster count, root region (fixed for 12/16,
  cluster chain for 32), **LFN reconstruction** (works on deleted entries — the
  char payload survives 0xE5), lowercase-flag 8.3 decoding, **first-char recovery
  for deleted 8.3 names via sibling common-prefix inference**, FAT32 first-cluster
  heuristic (high-word-zeroed search), **both FATs compared**, contiguous
  assumption on delete (`Suspect`).
- **fs-ntfs** (docs/plan/04 §3.3, from the public NTFS docs): boot sector, **MFT
  fix-ups**, record walk with the in-use flag (live/deleted), `$STANDARD_
  INFORMATION`/`$FILE_NAME` (prefers Win32 names)/`$DATA` (resident + non-resident
  **data runs**, compressed/sparse noted), `$Bitmap`, **orphan `FILE` scan** in
  unallocated space, and best-effort **`$I30` slack + `$UsnJrnl:$J`** name mining
  (`record.rs`, `usn.rs`, FILETIME→date in `date.rs`).
- Each crate: `probe()` (confidence), `open()`/`open_boxed()`,
  `allocation_bitmap()`, `walk()`, `read_extents()`; deny-unwrap/expect/indexing
  lints; per-parser randomized `fuzz_smoke` tests + libFuzzer targets.
- New read-only helper `reclaim_block::MemorySource` (in-memory `BlockSource`) for
  tests and fuzzing.

### D. Engine integration + merge (docs/plan/03 §3, §5)
- `reclaim-session::meta`: `run_quick` parses the scheme, builds an `OffsetView`
  per volume, probes the engine registry (NTFS/exFAT/FAT), walks the best match,
  scores each entry and stores it. `scan` (default) runs **quick then deep**;
  `--quick`/`--deep` select one pass.
- **Unified result store**: named entries and carved files share the `carved`
  table (new columns `path`, `state`, `kind`, `extents`, `merged`; Phase-1 DBs
  migrated with guarded `ALTER TABLE`). `recover` reads multi-extent named files.
- **Carver ↔ bitmap**: `--unallocated-only` labels carved results
  allocated/unallocated from the FS bitmap and hides the allocated ones.
- **Merge (§5 step 6)**: a carved range equal to a named entry's extents is
  marked `merged` and hidden — the named file (real path) supersedes it.
- `results --format tree` prints recovered paths as a directory tree;
  `--deleted-only` and `--engine` filters work; the table shows a `STATE` column.

### E. Golden images + named_recall scoring (docs/plan/09 §2, §3)
- `scripts/gen-fs-images` builds a **synthetic, spec-faithful NTFS** image
  (`ntfs-delete`) — this host has no `mkfs.ntfs`/Docker (see Decisions).
- The existing Phase-0 **exfat-camera-delete** and **fat32-usb-delete** images are
  real exFAT/FAT volumes with MBRs and are the exFAT/FAT `*-delete` targets.
- `scripts/bench.sh` + `bench_score.py` extended with **named_recall** (deleted
  file recovered at the right path with matching content, FAT first-char loss
  tolerated) alongside content_recall; `docs/build-log/phase-2/bench.md`:

  | image | fs | named_recall | content_recall | photorec | 
  |---|---|---|---|---|
  | exfat-camera-delete | exFAT | **100%** | 100% | 58.3% |
  | fat32-usb-delete | FAT32 | **100%** | 100% | 50.0% |
  | ntfs-delete | NTFS | **100%** | 100% | 66.7% |
  | apfs-delete-history | APFS | n/a (Phase 3) | 82.5% | 70.0% |
  | hfsplus-delete | HFS+ | n/a (Phase 3) | 100% | 85.0% |

  **named_recall ≥ 0.95 met on every Phase-2 filesystem.** content_recall ≥
  PhotoRec on every image and ≥ Phase 1 (FAT32 improved 85%→100%: the metadata
  engine recovers the deleted files' bytes exactly by extent, where the Phase-1
  statistical `.txt` carve fell short).

## Decisions made
- **NTFS: own parser, not the `ntfs` crate.** The permissive `ntfs` crate
  (ColinFinck) was evaluated. It reads *live* volumes well but exposes none of
  the deleted-record / orphan-`FILE` / `$I30` / `$UsnJrnl` mining that is the
  point of recovery, and would add a dependency while still requiring us to
  hand-roll all of that. We implement from the public NTFS documentation, matching
  the no-unwrap/read-only discipline of the other engines (build guide trap 3).
- **Synthetic NTFS golden image.** No `mkfs.ntfs` and no Docker on this host, so
  `ntfs-delete` is written by a spec-faithful builder in `scripts/gen-fs-images`
  (boot + `$MFT` + `$Bitmap` + live/deleted user records with non-resident
  `$DATA`). It exercises the engine end-to-end and gives a deterministic
  named_recall target; a real `mkfs.ntfs` image is a Ryker item.
- **Unified `carved` table for both carved and named results** (rather than a
  separate `entries` query path), so `results`/`recover`/`report` and the merge
  are uniform. Phase-1 sessions migrate via guarded `ALTER TABLE`; the
  deterministic-id resume gate still holds.
- **FAT deleted-name recovery**: the LFN char payload survives 0xE5, so long
  names come back intact; for lowercase-flagged 8.3 names (macOS writes user
  files this way) the lost first character is inferred from the common prefix of
  live siblings — this made FAT32 named_recall exact (1.0).
- **Merge rule = exact extent equality.** A carved `(offset,len)` collapses only
  when it equals a named entry's extent, so genuine carve-only finds are kept.
- **`scan` default = quick then deep**; extents are stored **absolute** so both
  passes share one coordinate space.

## Gates
- `cargo ci` **green** on macOS: `fmt --check`; `clippy --workspace --all-targets
  -D warnings`; `cargo test --workspace` (**~180 tests** incl. the three golden
  integration tests that skip in CI, the four `fuzz_smoke` suites, and the Phase-1
  scan/resume determinism gate — still passing with the new columns); `cargo deny`
  (licenses/advisories/bans/sources ok); `check-readonly` (no new write paths — the
  engines and `reclaim-part` are read-only; only the allow-listed session/report/CLI
  writers remain). The GitHub Actions macos+ubuntu matrix should be re-run on the
  pushed `p2:` commits.
- **Fuzzing:** cargo-fuzz is **not installed** in this environment, so — as in
  Phase 1, where the nightly fuzz job was local-only — the six libFuzzer targets
  (`crates/reclaim-fs-fuzz`: part_scan, exfat, fat, ntfs, ntfs_record, ntfs_usn)
  are written and ready for `scripts/fuzz-fs.sh` on a nightly runner. The
  CI-runnable stand-in is the in-tree `fuzz_smoke` tests: **tens of thousands of
  random + header-seeded inputs per parser, 0 panics** (part ~5k, exFAT/FAT ~6k
  each, NTFS record 20k + USN/$I30 10k + volume 4k). These run in every
  `cargo test`.
- **Merge gate:** quick+deep on the exFAT image → 101 named results, **0 carved
  duplicates of a named file** (all 48 contiguous carves collapsed into their
  named entries).
- **Benchmark:** `docs/build-log/phase-2/bench.md` written; named_recall ≥ 0.95
  on exFAT/FAT/NTFS, content_recall ≥ PhotoRec and ≥ Phase 1 on every image.

## Manual items (Ryker) — none block Phase 3
1. **Real NTFS image (optional):** with Docker/OrbStack running,
   `mkfs.ntfs` an image and drop it at `testdata/build/ntfs-delete-real.img` to
   corroborate the synthetic one. The engine already parses real MFT layouts.
2. **SD-card spot check (build guide Phase-2 "Your check"):** delete three photos
   on the sacrificial SD card, `sudo reclaim scan disk4 --session ~/s2`, then
   `reclaim results ~/s2 --deleted-only --format tree` shows the DCIM paths with
   correct names/dates; `reclaim recover ~/s2 ~/Recovered --deleted-only
   --preserve-paths --verify` and open a recovered file.
3. **CI runners:** re-run the macos + ubuntu matrix on the pushed `p2:` commits.

## Known gaps (deferred, with phase)
- **APFS / HFS+ metadata** — Phase 3 (their partitions are detected here but not
  walked; named_recall shows `n/a`).
- **NTFS `$I30` slack + `$UsnJrnl:$J`** are best-effort name miners (parsers
  written + fuzzed); they enrich names for records whose `$FILE_NAME` is gone but
  do not yet reconstruct content, and the synthetic image does not exercise them.
- **NTFS compressed/sparse `$DATA`** is flagged and scored lower but not
  decompressed (round 1 scope).
- **Deleted-directory recursion**: engines recurse into live directories only; a
  wholly-deleted directory's children are recovered only if reachable by scan.
- **Streaming for huge volumes**: `run_quick` buffers a volume's entries before a
  chunked insert (fine for the golden images; bounded by `WalkOpts::max_entries`).
  Revisit with the nightly big-image test (Phase 6).
- **No doc 04 factual errors found.** Empirically confirmed doc 04 §3.4: macOS
  preserves the FAT32 first-cluster high word on delete (Windows zeroes it), and
  writes 8.3-compatible user files with the lowercase NT flags rather than LFNs.

## Next phase needs (facts Phase 3 should not rediscover)
- **Crates:** `reclaim-part` (`scan`, `PartitionMap`, `Container`),
  `reclaim-fs-core` (`FileSystem`/`Entry`/`Bitmap`/`EntrySink`/`Probe`/`WalkOpts`/
  `score`), `fs-exfat`/`fs-fat`/`fs-ntfs` (`probe`/`open`/`open_boxed`),
  `reclaim-session::meta` (`run_quick`, engine registry, `CollectSink`,
  `AbsBitmap`), `Store::{insert_entries, merge_carved_into_entries, label_carved}`.
- **Adding an engine** = implement `FileSystem` + `probe`/`open_boxed`, then add a
  line to `engines()` in `reclaim-session/src/meta.rs`. APFS/HFS+ partitions are
  already detected by `reclaim-part` (containers list + GPT type GUIDs) and will
  arrive as `OffsetView`s.
- **Golden images:** `scripts/gen-fs-images` (synthetic NTFS) and the Phase-0
  `gen-images` (hdiutil exFAT/FAT/APFS/HFS+). Ground-truth sidecars score by
  **path + sha256** (the Phase-0 `F_LOG2PHYS` extents are volume-relative, and the
  APFS/HFS+ ones are unusable — a struct-packing bug in `gen-images`); Phase 3
  APFS scoring should also match on path+hash, not the sidecar extents.
- **Result model:** one `carved` table; named entries carry real `path`,
  `state`, and absolute `extents` (JSON). `recover` handles multi-extent files.
