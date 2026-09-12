# Phase 3 — APFS + HFS+ metadata recovery, snapshots

- **Date:** 2026-09-12
- **Branch:** `dev/v1` (off `main`)
- **Commit range:** the `p3:` commits on `dev/v1` (`git log f42ac66..HEAD`).
- **Toolchain:** rustc/cargo **1.98.1** (pinned); nightly + cargo-fuzz **0.13.2**
  for the libFuzzer targets. In-tree `fuzz_smoke` tests are the CI-runnable
  robustness proof.
- **Sources for the parsers:** *Apple File System Reference* (APFS) and Apple
  **TN1150** (HFS+) only. `apfs-fuse` (GPL) and `libfsapfs` (LGPL) were read to
  understand the format; **no GPL/LGPL code was copied** (build guide Part 2.3
  trap 3, Part 1.4 rule 2) — `cargo deny` stays green with no new dependencies.
- **Ground truth:** decoded live from the real Phase-0 golden images
  (`testdata/build/apfs-delete-history.img`, `hfsplus-delete.img`) before writing
  a line of Rust; every offset in the parsers is verified against those bytes.

## Done

### A. fs-apfs — APFS metadata engine (docs/plan/04 §3.1, steps 1–8 except encryption)
- `crates/fs-apfs`, implemented from the Apple File System Reference:
  - **`obj.rs`** — `obj_phys_t` header + **Fletcher-64** verification (modulo
    2³²−1 over the 32-bit words after the checksum field; validated against all
    197 FS-tree nodes of the golden container), bounds-checked LE readers.
  - **`btree.rs`** — one generic `btree_node_phys_t` walker for both tree shapes:
    fixed-KV (object map, children by paddr) and variable-KV (`FSTREE`, children
    by *virtual oid* resolved through the volume omap). Visited-node dedup +
    a node budget bound a crafted/looping tree.
  - **`records.rs`** — `j_key` split (obj-id / type), `j_inode` (name +
    `j_dstream` size via the `INO_EXT_TYPE_*` extended-field blob), `j_drec`
    (hashed and non-hashed key layouts), `j_file_extent` (len masked to 56 bits,
    phys block → offset), `j_xattr` name (decmpfs / ResourceFork recognised),
    `apfs_snap_metadata`.
  - **`spaceman.rs`** — the space-manager allocation bitmap: ephemeral spaceman
    resolved through the newest checkpoint-map, its main device → chunk-info
    block → per-chunk bitmap blocks assembled into one packed bitmap (single-CIB
    layout, ≤ ~16 GiB containers — all test volumes; multi-CAB returns `None`).
  - **`lib.rs`** — `probe`/`open`/`open_boxed`; **checkpoint descriptor ring**
    enumeration → every valid `NXSB` by `xid` (the history); container omap →
    `apfs_superblock_t` per volume; volume omap → FS-tree walk; **diff** against
    older checkpoints and snapshots (present-then-absent ⇒ Historical); **orphan
    scan** of every non-live block for checksum-valid FS-tree leaf nodes;
    `snapshots()` and the `recovery_points()`/`diff_from()` API for the CLI.
  - Volume roles: the sealed **System** volume is skipped by default
    (`WalkOpts::include_system_volume` includes it); Data volumes are the target.
    Encrypted volumes: read only via the OS-unlocked device — **no own crypto**
    (build guide Part 3.6); nothing here decrypts.

### B. fs-hfs — HFS+/HFSX metadata engine (docs/plan/04 §3.2, from TN1150)
- `crates/fs-hfs`:
  - **`record.rs`** — big-endian `HFSPlusForkData`, `HFSPlusCatalogKey`,
    file/folder/thread records; bounds-checked BE readers.
  - **`date.rs`** — HFS+ 1904-epoch timestamps → `YYYY-MM-DD`.
  - **`lib.rs`** — volume header at +1024 with **alternate-VH fallback** at
    `size−1024`; catalog B-tree header node → node size + leaf chain; live
    file/folder walk with **path resolution** via catalog keys; **allocation
    file** → bitmap (HFS+ is MSB-first per byte, reversed to the crate's
    LSB-first `Bitmap`); **whole-volume stale-record scan** that recovers deleted
    `kHFSPlusFileRecord`s from the **journal** and freed catalog nodes (journaled
    HFS+ compacts a node on delete, but previous node images survive there — the
    node-slack idea generalised to the whole volume). HFSX (`HX`) case-sensitive
    volumes handled (signature differs only).
  - A plausibility filter (valid dates, plausible CNID, filename-like name, an
    in-volume extent) keeps the raw-byte sweep from emitting false positives.

### C. CLI + planner + mounted source
- **Engine registry**: `reclaim-session::meta::engines()` now probes APFS and
  HFS+ ahead of NTFS/exFAT/FAT; `reclaim scan <image|disk>` walks an APFS
  container or HFS+ volume automatically (partition windows from Phase-2's
  `reclaim-part`).
- **`reclaim snapshots <SOURCE>`** — lists the container's volumes, reachable
  **checkpoint xids** (newest first) and any APFS snapshots.
  **`reclaim snapshots <SOURCE> diff --from <xid>`** — lists files present at that
  snapshot/checkpoint but absent now. (`crates/reclaim-cli/src/commands/snapshots.rs`.)
- **`mounted:/path`** — a read-only POSIX-walk source for Trash recovery
  (docs/plan/06 §6, §8): `reclaim scan mounted:/Volumes/Foo` enumerates live
  files and marks anything under `.Trash`/`.Trashes`/`Trash` deleted; `recover`
  copies those by path (never follows symlinks; refuses a same-volume
  destination). No block device is opened.

### D. Golden images + bench (docs/plan/09 §2, §3)
- `scripts/gen-images` extended with fraction/stride deletes, clonefile (`cp -c`)
  + transparent compression (`ditto --hfsCompression`), and HFS+ journal churn
  (run **before** the target deletes so they are the journal's freshest entries).
  New recipes: **apfs-many-deletes** (2000 files, delete 1000 as the last op),
  **apfs-clone-compress**, **apfs-snapshots**, **hfsplus-journal-history**,
  **hfsplus-case-sensitive**.
- **named_recall** (deleted file recovered at the right path with matching
  SHA-256) measured by `scripts/bench_score.py`; APFS/HFS+ score by **path + hash**
  (the Phase-0 sidecar extents are unusable — a known `gen-images` packing bug).

  Full table + PhotoRec comparison: `docs/build-log/phase-3/bench.md`
  (`scripts/bench.sh`, content_recall = exact SHA-256, named_recall = deleted
  file recovered at the right path + content).

  | image | fs | named_recall | content_recall | photorec |
  |---|---|---|---|---|
  | apfs-many-deletes | APFS | **100%** | 100% | 71.3% |
  | apfs-snapshots | APFS | 100% | 100% | 75.0% |
  | apfs-clone-compress | APFS | 100% | 100% | 66.7% |
  | apfs-delete-history | APFS | **0%** \* | 87.5% | 70.0% |
  | hfsplus-delete | HFS+ | **100%** | 100% | 85.0% |
  | hfsplus-journal-history | HFS+ | 100% | 100% | 75.0% |
  | hfsplus-case-sensitive | HFSX | 100% | 100% | 75.0% |
  | exfat-camera-delete | exFAT | 100% | 100% | 58.3% |
  | fat32-usb-delete | FAT32 | 100% | 100% | 50.0% |
  | ntfs-delete / -quick-format | NTFS | 100% | 100% | 66.7% |

  \* apfs-delete-history — the documented overwrite case (below). content_recall
  ≥ PhotoRec on **every** image; content is exact by extent for the named files.

  **Targets:** apfs-many-deletes ≥ 0.90 — **met (1.00)**; hfsplus-delete ≥ 0.95 —
  **met (1.00)**. apfs-delete-history ≥ 0.90 — **not met (0.00)**, explained
  under *Checkpoint depth & the overwrite limit* with numbers (as the prompt
  and gate permit).

## Decisions made
- **Own APFS/HFS+ parsers, no external crate.** No permissive, recovery-oriented
  APFS/HFS+ crate exists; the GPL/LGPL implementations may only be read. Both
  engines are written from the primary Apple specs with the workspace's
  no-unwrap/no-index/read-only discipline. `cargo deny` unchanged (no new deps).
- **The FS tree is virtual; its internal-node children are oids, not paddrs.**
  Verified on the real image — the omap must resolve every FS-tree child. One
  generic B-tree walker serves both trees via a `fetch` closure.
- **Orphan scan examines every non-live block, not just space-manager-free
  blocks.** The decisive Phase-3 finding: when APFS frees a metadata node it is
  quickly **re-allocated** (to new metadata) while the *old bytes survive*, so a
  scan restricted to spaceman-free blocks misses most deleted records. Scanning
  every block that is not referenced by a live file (checksum-gated on
  `obj_phys` + FS-tree subtype) took apfs-many-deletes from **8% → 100%**.
- **Scoring uses a live-referenced bitmap, not the raw space-manager bitmap.**
  The space-manager marks a freed-then-reallocated block *allocated* even though
  its old data is intact and recoverable, which drove genuinely-recoverable files
  to score 0. The real "overwritten?" signal is *"is this block referenced by a
  live file now?"* — a bitmap built from the current volume's file extents. The
  space-manager bitmap is still parsed (spec deliverable) and used to report free
  space and could steer a future carve.
- **Extent dedup + logical-size trim.** The orphan scan aggregates records from
  every surviving COW copy of a node, so the same extent appears many times;
  keeping one per logical address and trimming to the inode's logical size made
  content exact.
- **Snapshot creation on a detached image is not possible without root.**
  `fs_snapshot_create(2)` returns `EPERM` for a non-root caller
  (`scripts/apfs-snapshot-helper/snap.c`), and `tmutil localsnapshot` creates a
  *purgeable* snapshot that `diskutil apfs listSnapshots` does not retain. As the
  prompt allows, **apfs-snapshots relies on checkpoint history** (a small delete
  kept in-ring); the snapshot *diff* code path is exercised by the snapshot
  enumeration + `diff_from` API and the `snapshots` command. Creating a
  persistent snapshot for a golden image is a Ryker item (needs `sudo`).
- **Fuzz-hardening (all fuzz-found).** A crafted `nx_xp_desc_index` add-overflow
  panic → `wrapping_add`; unbounded `xp_desc_blocks`/`xp_desc_len` loops → capped
  at `MAX_DESC_RING`; a volumes×checkpoints×tree blow-up → one shared node-visit
  budget + visited-node dedup. After each fix the crashing/timeout input was kept
  as a regression seed and re-run clean.

## Gates
- `cargo ci` **green** on macOS: `fmt --check`; `clippy --workspace --all-targets
  -D warnings`; `cargo test --workspace` (adds the fs-apfs/fs-hfs unit +
  golden-image + `fuzz_smoke` suites); `cargo deny` (no new deps); `check-readonly`
  (the two engines are read-only; the only new write paths are the mounted
  `recover` copy — inside the already-allow-listed `recover.rs` — and the session
  store). Re-run the GitHub Actions macos+ubuntu matrix on the pushed `p3:` commits.
- **Fuzzing:** four new libFuzzer targets (`apfs`, `apfs_record`, `hfs`,
  `hfs_record` in `crates/reclaim-fs-fuzz`) each **ran ≥ 195 s clean — 0 crashes,
  0 timeouts, no artifacts** (apfs 135k execs / cov 442, apfs_record 52.7M,
  hfs 43k / cov 686, hfs_record 42.2M). `scripts/fuzz-fs.sh` re-runs them. The
  CI-runnable stand-in is `cargo test -p fs-apfs -p fs-hfs --test fuzz_smoke`:
  tens of thousands of random + header-seeded inputs per parser, 0 panics.
- **Bench:** table above; named_recall targets met on apfs-many-deletes (1.00)
  and hfsplus-delete (1.00); apfs-delete-history explained with numbers.
- **`snapshots` works against a real local snapshot, read-only, non-sudo.** On
  the M0 host, `reclaim snapshots disk3` (the live boot container) listed its 5
  volumes, ~139 reachable checkpoint xids, and the real Time Machine local
  snapshot `com.apple.TimeMachine.2026-09-12-182006.local` (xid 1545111);
  `reclaim snapshots disk3 diff --from 1545111` then walked that snapshot vs the
  current Data volume read-only and correctly reported no deletions in the
  window. No `sudo` was needed — Full Disk Access is effective for this shell's
  responsible process (docs/plan/06 §12), and the raw container read succeeded.
  It also works against the golden container's checkpoint history
  (`reclaim snapshots testdata/build/apfs-delete-history.img` → 4 xids, 1 volume).
  Note the *real* boot container's descriptor ring is far deeper (~139
  checkpoints) than the small 512 MiB golden images (~4) — ring depth scales with
  `nx_xp_desc_blocks`.

## Checkpoint depth & the overwrite limit (numbers)
- The **checkpoint descriptor ring is shallow**: `nx_xp_desc_blocks` on the
  golden containers is 8 blocks, holding **~4 checkpoint superblocks** (one
  checkpoint-map + one NXSB per transaction). apfs-delete-history's reachable
  xids are **{366, 365, 364, 363}** — depth **4**, *not* the 300+ the recipe's
  churn count suggests. **Churn does not deepen recoverable history; it destroys
  it.** This corrects the apfs-delete-history recipe's premise and doc 04 §3.1
  step 2. (Ring depth scales with `nx_xp_desc_blocks`: the live boot container
  observed ~139 reachable xids, so large real containers have far more history.)
- apfs-delete-history deletes file_000..011 and *then* runs **320** create/delete
  churn transactions. Those 320 transactions re-allocate and overwrite the freed
  metadata *and* data blocks, so 0/12 target files are recoverable by any method
  (checkpoint diff, snapshot, or orphan scan) — an honest, realistic
  busy-volume/TRIM outcome (docs/plan/06 §3). The same run still recovers **58**
  *other* deleted artifacts (the later `.churn_*.tmp` files) whose blocks were not
  yet reused, which is why the engine is demonstrably working.
- apfs-many-deletes shows the opposite, common case — a bulk delete with no
  subsequent churn — where the orphan scan recovers **1000/1000** with exact
  content. The lesson: APFS deleted-file recovery is governed by *subsequent
  writes*, not by delete count.

## Manual items (Ryker) — none block Phase 4
1. **Persistent APFS snapshot golden image (optional):** with `sudo`, run
   `scripts/apfs-snapshot-helper/snap /Volumes/<vol> <name>` between populate and
   delete in `gen-images` to build a real `apfs_snap_metadata` snapshot; the
   engine's snapshot diff then recovers a full older view (≈100%). Not required —
   the checkpoint-history path is exercised without it.
2. **`snapshots` against the live boot container** already works on the M0 host
   non-sudo (FDA effective; see Gates). On a host where the DAC gate blocks the
   raw read, add the user to the `operator` group per docs/plan/06 §12 first.
3. **Re-run the CI matrix** on the pushed `p3:` commits.

## Known gaps (deferred, with phase)
- **decmpfs / resource-fork content is not decompressed** (round-1 scope, build
  guide Part 3.6): compressed files' inline `com.apple.decmpfs` data and
  resource-fork forks are recognised but a compressed file's *content* is not
  reconstructed. apfs-clone-compress verifies the walk handles them without error
  and recovers the uncompressed/cloned files.
- **Extents-overflow B-tree (HFS+)** and the APFS **extent-reference tree** are
  not consulted for very fragmented deleted files (>8 inline HFS+ extents); the
  inline extents are used. Revisit if a fragmented-file corpus needs it.
- **Space-manager multi-CAB layout** (containers > ~16 GiB) returns no bitmap;
  scoring then falls back to the live-referenced map built during the walk.
- **Huge-volume streaming**: the orphan sweep reads every non-live block; bounded
  by `MAX_ORPHAN_SCAN_BLOCKS` (≈32 GiB) — a full multi-TB sweep is a Phase-6
  streaming item (carried over from Phase 2).
- **APFS snapshot creation for images** needs root (see Decisions).

## Next phase needs (facts Phase 4 should not rediscover)
- **Crates:** `fs-apfs` (`probe`/`open`/`open_boxed`, `Apfs::{recoverable_xids,
  recovery_points, diff_from, volume_names}`, `RecoveryPoint`/`RecoveryKind`) and
  `fs-hfs` (`probe`/`open`/`open_boxed`) both implement `FileSystem` and are wired
  into `reclaim-session::meta::engines()`.
- `WalkOpts` gained `include_system_volume` (default false; non-APFS engines
  ignore it).
- **`mounted:` source**: handled in `reclaim-cli::commands::scan` +
  `reclaim-session::meta::run_mounted`; recover branch in `recover.rs` keys off a
  `mounted:{root}` `source_id`.
- **Golden images:** all built by `scripts/gen-images` from `testdata/recipes`;
  score by **path + sha256**, never the sidecar extents.
- **APFS on-disk offsets** are recorded inline in the module doc-comments and in
  the private memory note; the FS tree's children are virtual oids (omap-resolved).
