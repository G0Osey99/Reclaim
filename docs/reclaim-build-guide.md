# Reclaim — Round 1 Build Guide & Phased Plan (v1.0: CLI core + Mac GUI)

Written 2026-09-12 from the 13-document planning set in `docs/plan/` (`00-README.md` … `12-glossary.md`, the "Reclaim build documentation set"). Target repo: `reclaim/` — **new and empty**; Ryker creates it and drops the planning docs into `docs/plan/` before Phase 0 (Part 5.1).

This is the operating manual for round 1. Round 1 takes the project from nothing to **Reclaim 1.0: a read-only Rust recovery core, the `reclaim` CLI, and a notarized SwiftUI app** — carving, Tier-1 filesystem metadata recovery (APFS, HFS+, NTFS, exFAT, FAT), lost-volume search, imaging, sessions, previews. Fragment reassembly, RAID, ext-family polish beyond detection, and own-crypto unlock are **round 2** (doc 10, M6–M7). Each phase is one `/goal` prompt for a fresh Claude Code session; everything that needs a human is collected in **Part 5**.

---

## Part 1 — How to run this plan

### 1.1 The loop

For each phase, in order:

1. Open a fresh Claude Code session in `reclaim/` **on the Mac** (the core is cross-platform but enumeration, `hdiutil`, `diskutil`, signing and the GUI need macOS), with Opus 4.8 (or the current best model) selected and the session's multi-agent setting on (via `/config`, not a keyword).
2. Paste the phase prompt from Part 4 **exactly as written**. Every prompt begins with `/goal` so the Stop hook keeps the session working until the gates pass. **Each prompt is ≤ 4000 characters and contains no `ultracode` keyword** — keep it that way if you edit one.
3. When the session reports done, read `docs/build-log/phase-N.md` (the agent writes it) and do the manual items it lists.
4. Run the phase's **Your check** list (Part 4) yourself. Ten to twenty minutes.
5. Move to the next phase.

Phases are sequential; each prompt reads the previous build log so state carries without re-explaining.

### 1.2 Why phases are ordered this way

The planning set's thesis (doc 02 §7, doc 10): build the **read-only block layer and the carver first** — that alone is a PhotoRec-class tool and it is testable against image files on any machine — then add filesystem metadata engines in order of who loses data (camera media → Windows externals → Mac volumes), then the structures that make damaged media recoverable, and only then the GUI, which is a thin client and must not drive the core's design.

| Phase | Name | Unblocks |
|---|---|---|
| 0 | Workspace, block layer, enumeration, platform proofs | a repo that lists disks read-only, a golden-image generator, answers to every `[verify]` in doc 06 |
| 1 | Carving engine, imager, sessions, CLI | `reclaim scan --deep / results / recover / image` — a usable carver ≥ PhotoRec |
| 2 | Partition tables + exFAT / FAT / NTFS metadata | named, pathed recovery for cameras and Windows drives; merge with carved results |
| 3 | APFS + HFS+ metadata, snapshots | the Mac differentiator: checkpoint/snapshot history, catalog slack, journal |
| 4 | Lost volumes, ext family, image containers, health, Recovery-Mode build | damaged/re-partitioned media, DMG/VMDK/E01 sources, image-first nudging, boot-disk story |
| 5 | SwiftUI app, privileged helper, previews, notarization | Reclaim.app (beta) installable on any Mac |
| 6 | Benchmark, hardware QA, docs, release 1.0.0 | tagged release, Homebrew, published recovery-rate table |

### 1.3 Branching

- Round 1 ships **1.0.0**. Default branch `main`; all phase work on `dev/v1`; CI runs on `main`, `dev/**`, and PRs.
- Phases 0–5 commit to `dev/v1` in small, phase-tagged commits (`p0: …`, `p1: …`) and push. Each phase may open an intermediate PR if Ryker wants review, but the plan assumes one PR `dev/v1 → main` in Phase 6.
- Tags: `v0.1.0` at the end of Phase 1 (first usable carver), `v0.5.0-beta` at the end of Phase 5 (GUI beta), `v1.0.0` in Phase 6. Pre-1.0 tags exist so `cargo install --git … --tag` and Homebrew formula testing have something to point at.
- Nothing is "deployed" — this is a desktop tool. Phase 6 publishes a GitHub Release with the notarized `.dmg`, CLI tarballs (arm64 + x86_64 + universal), and a Homebrew tap formula.

### 1.4 Working rules every prompt enforces

1. **Read-only sources, by construction.** The `BlockSource` trait has no write method; nothing outside `reclaim-block/src/imaging/dest.rs` and `reclaim-session` may open anything for writing. A CI grep enforces the allow-list (doc 09 §5). Every integration test hashes its source image before and after. A violation is a P0 and blocks the phase.
2. **No GPL/LGPL code copied.** PhotoRec/TestDisk, Sleuth Kit, libfsapfs, apfs-fuse, dislocker are read for understanding only; signatures and validators are written from primary format specs (doc 11 §2). `cargo deny` with the repo's allow-list must pass. Running PhotoRec in CI as a benchmark competitor is fine.
3. **Corruption is the normal input.** Parser crates carry `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]`; every on-disk struct is parsed with bounds checks; every parser gets a `cargo fuzz` target before the phase closes. No panics on data.
4. **Decide, don't ask.** Make routine calls (crate choice, struct names, test layout) and record them in the build log under *Decisions made*; stop only for Part 5 items.
5. **Read before writing.** Read `docs/plan/00-README.md`, the documents the prompt names, and the prior build log first. Verify Apple/IOKit/SMAppService/UniFFI APIs and crate versions against current docs, not memory.
6. **Docs are the spec, but the spec can be wrong.** Where a planning doc conflicts with reality (a magic number, an API, a [verify] experiment), fix the doc in the same commit and note it under *Decisions made*. Doc 06's `[verify]` items are resolved in Phase 0 and the markers removed.
7. **Never write to a real device in tests; never test on media Ryker cares about.** Phase work uses generated images (doc 09 §2) and the sacrificial test media in Part 5.2. `sudo` prompts are Ryker's to answer (Part 5.1).
8. **Gates before done:** `cargo ci` (fmt, clippy -D warnings, test, deny, readonly-grep) green on macOS and Linux; the phase's tests/benchmarks pass; the build log is written; the branch is committed and pushed.

### 1.5 Build log format

Every phase ends by writing `docs/build-log/phase-N.md`:

```
# Phase N — <name>
Date, branch, commit range, toolchain versions.
## Done              bullets, with crate/file paths, grouped under the prompt's A./B./C. scope
## Decisions made    each one a line with the reason (incl. any planning-doc corrections)
## Gates             output summary of cargo ci / fuzz minutes / benchmark table
## Manual items      what Ryker must do next, with the exact command or screen
## Known gaps        anything deferred, and to which phase
## Next phase needs  facts the next prompt should not have to rediscover (device names, sizes, paths)
```

---

## Part 2 — Ground truth

Read `docs/plan/00-README.md` once in full and skim the rest. This is the compressed version the prompts refer back to.

### 2.1 What already exists — do NOT redo

Only the plan. Thirteen markdown documents in `docs/plan/`:

| Doc | Use it for |
|---|---|
| 01 competitive analysis | the feature checklist (§2) Reclaim is measured against |
| 02 PRD | FR-/NFR- ids every prompt cites; key decisions (Rust core, CLI first, SwiftUI, SQLite sessions, Apache-2.0) |
| 03 architecture | `BlockSource` / `FileSystem` traits, engine pipeline, crate layout (§4), concurrency (§3), security model (§7) |
| 04 filesystem matrix | tiers, probe anchors, per-FS deleted-entry methods; APFS algorithm §3.1; image containers §5 |
| 05 signature catalog | TOML schema (§1), dedup rules (§2), ~150 seed signatures (§3), validators to write first (§4), reassembly (§5 — round 2) |
| 06 macOS platform notes | raw access, root vs FDA, T2/Apple Silicon, FileVault, TRIM, SMART, signing; **`[verify]` items resolved in Phase 0** |
| 07 CLI spec | command grammar, NDJSON events, exit codes, session layout |
| 08 GUI spec | screens, states, FFI surface |
| 09 testing | recipe format, scoring, fuzz targets, read-only proof, hardware checklist |
| 10 roadmap | M0–M7; round 1 = M0–M5 |
| 11 safety/legal | licensing rules, `cargo deny` policy, EULA text points |
| 12 glossary | vocabulary |

No code, no CI, no signing identity, no test media until Part 5.1/5.2 are done.

### 2.2 The phase ↔ doc map

| Phase | Primary docs | Crates created / touched |
|---|---|---|
| 0 | 03 §2.1, §4, §7; 06 (all); 09 §2, §5 | workspace, `reclaim-block`, `reclaim-platform-macos`, `reclaim-cli` (list/info/doctor), `testdata/` generator |
| 1 | 05 §1–§4; 03 §2.3, §2.5, §2.6, §3; 07; 09 §3 | `reclaim-sigs`, `reclaim-carve`, `reclaim-session`, `reclaim-block/imaging`, `reclaim-report`, CLI scan/results/recover/image |
| 2 | 04 §1, §2 (exFAT/FAT/NTFS rows), §3.3–§3.4; 03 §2.2 | `reclaim-part`, `reclaim-fs-core`, `fs-exfat`, `fs-fat`, `fs-ntfs` |
| 3 | 04 §3.1–§3.2; 06 §6 | `fs-apfs`, `fs-hfs`, CLI `snapshots` |
| 4 | 04 §2 (ext/ISO rows), §5; 03 §2.4; 06 §4, §7, §8 | `reclaim-structs`, `fs-ext`, `fs-iso`, image containers, health, static CLI build |
| 5 | 08 (all); 06 §2, §9; 03 §7 | `reclaim-ffi`, `reclaim-preview`, `apps/Reclaim.app`, helper |
| 6 | 09 §3, §6, §7; 11 §7 | benchmarks, docs, release tooling |

### 2.3 The four traps

1. **Root and Full Disk Access are separate gates, and both are interactive.** Reading `/dev/rdiskN` needs root (`sudo`); reading the raw device behind the *boot* volume also needs the terminal app granted Full Disk Access. An agent cannot type Ryker's password or click System Settings. Phase 0 therefore tests raw access against **external sacrificial media and image files first**, and the boot-disk experiments are run with Ryker at the keyboard (Part 5.1). Never let a prompt stall on a `sudo` it cannot answer — the prompt says what to do instead.
2. **`hdiutil`-built APFS/HFS+ images are real, but the macOS kernel owns them while attached.** The generator must `hdiutil detach` before the image is handed to a test, and APFS checkpoint history only accumulates if the generator actually performs many transactions (create/delete loops) before detaching. exFAT/FAT32 images come from `hdiutil create -fs …` too; NTFS and ext4 images need a Linux container (`mkfs.ntfs`, `mkfs.ext4` via Docker/OrbStack) — Phase 2/4 assume Docker is present (Part 5.1).
3. **Apple's APFS spec is the only allowed APFS source for code.** `apfs-fuse` (GPL) and `libfsapfs` (LGPL) may be read to understand; the agent must implement from *Apple File System Reference* and write its own tests. Same for NTFS (use the permissive `ntfs` crate if it fits; otherwise implement from the public NTFS documentation) and HFS+ (Apple TN1150).
4. **Signing and notarization need Ryker's Apple Developer account.** Phase 5 builds and signs locally only if `DEVELOPMENT_TEAM` and a Developer ID certificate exist in the keychain (Part 5.3); otherwise it produces an ad-hoc-signed build, documents the exact `codesign`/`notarytool` commands, and Phase 6 runs them once the account exists. Phase 5 must not block on this.

### 2.4 Running things on the Mac — what agents may do directly

- Build, test, fuzz, `hdiutil create/attach/detach`, `diskutil list`, `diskutil info`, reading **attached image files** and **sacrificial external media** (Part 5.2) with `sudo` when Ryker is present.
- **May not**: write to, erase, or `diskutil` any disk that is not on the Part 5.2 sacrificial list; disable SIP; install kexts; change Privacy settings. Read-only commands (`diskutil list`, `system_profiler`, `tmutil listlocalsnapshots`) are always fine.
- Benchmarks against PhotoRec: `brew install testdisk`, run `photorec /log /d out /cmd image.img search` in CI-style scripts on generated images.

---

## Part 3 — Locked decisions

Decided now so no phase re-litigates them.

### 3.1 Stack
Rust stable (pinned in `rust-toolchain.toml`), Cargo workspace per doc 03 §4; `clap` + `indicatif` + `serde_json` CLI; `rusqlite` (bundled) session store; `aho-corasick`/`memchr` matcher; `blake3` hashing; `uniffi` for Swift bindings; SwiftUI (macOS 13+) app with `SMAppService` helper. No Electron/Tauri. No async runtime in the core (threads + `crossbeam-channel`).

### 3.2 Read-only enforcement
`reclaim-block` exposes `BlockSource` with `read_at` only. Writes exist in exactly two places: `reclaim-block/src/imaging/dest.rs` (image output) and `reclaim-session` (SQLite + recovered files). `scripts/check-readonly.sh` greps for `write(true)`, `OpenOptions::create`, `pwrite`, `libc::write`, `ioctl` outside an allow-list file and fails CI on a hit. `reclaim --prove-readonly` logs every `open()` with flags.

### 3.3 Licensing
Apache-2.0 (`LICENSE`), `cargo deny` allow-list = MIT/Apache-2.0/BSD-2/BSD-3/ISC/Zlib/Unicode-3.0/MPL-2.0 (MPL only if dynamically separable). LGPL dependencies are permitted only as dynamically linked system libraries that the build does not vendor (none planned in round 1). `THIRD_PARTY.md` generated by `cargo about` in Phase 6.

### 3.4 Signatures are data
`crates/reclaim-sigs/catalog/*.toml` per doc 05 §1 schema; `build.rs` compiles them. Validators live in `reclaim-carve/src/validators/<name>.rs` and are registered by name. Adding a header-only format is a TOML PR with a sample file under `testdata/samples/` (≤ 64 KiB each, synthetic or public-domain).

### 3.5 Sessions and results
One SQLite file per session (doc 03 §2.5); deterministic result ids `blake3(source_id || engine || offset || len)`; checkpoint every 5 s or 1 GiB; events are NDJSON with the shapes in doc 07 §2. The GUI reads the same file — no second data model.

### 3.6 What round 1 does NOT do
No fragment reassembly (carve contiguous only; emit `Truncated`), no RAID engine, no BitLocker/LUKS/FileVault own-crypto (rely on the OS-unlocked device), no filesystem *repair*, no Windows/Linux GUI, no telemetry. ext2/3/4 and ISO get metadata listing in Phase 4 only as far as doc 04 Tier 2 describes; XFS/Btrfs/ZFS/UFS are detect-and-carve.

### 3.7 Versioning and release artifacts
`v0.1.0` (Phase 1), `v0.5.0-beta` (Phase 5), `v1.0.0` (Phase 6). Artifacts: `reclaim-<ver>-{aarch64,x86_64,universal}-apple-darwin.tar.gz`, `reclaim-<ver>-x86_64-linux.tar.gz` (CLI only), `Reclaim-<ver>.dmg` (notarized), `SHA256SUMS`. Homebrew: a tap repo `homebrew-reclaim` with a formula (CLI) and a cask (app).

---

## Part 4 — Phase prompts

Copy each block verbatim into a fresh session. Each is ≤ 4000 characters and begins with `/goal` (no `ultracode`).

---

### Phase 0 — Workspace, block layer, enumeration, platform proofs

**Goal:** a Cargo workspace with CI and the read-only lint, a `BlockSource` that reads raw devices and image files, macOS disk enumeration, `reclaim list / info / doctor`, a golden-image generator for exFAT/FAT32/APFS/HFS+, and every `[verify]` in doc 06 answered. No engines yet.

**Before you run it (manual):** Part 5.1 (repo + docs in `docs/plan/`, toolchain, Docker, sacrificial media plugged in, `sudo -v` in the terminal you'll run Claude Code from).

**Prompt:**

```
/goal Phase 0 of the Reclaim build: workspace, block layer, macOS enumeration, platform proofs. Read docs/plan/00-README.md, 03-architecture.md (§2.1, §3, §4, §7), 06-macos-platform-notes.md (all), 09-testing-and-validation.md (§2, §5), and reclaim-build-guide.md Parts 1.4, 2.3, 2.4, 3.1-3.3. Repo is empty except docs/. No recovery engines this phase.

A. Workspace + CI
- cargo workspace per doc 03 §4 with ONLY these crates now: reclaim-block, reclaim-platform-macos, reclaim-cli (bin `reclaim`). rust-toolchain.toml pinned stable. Workspace lints: clippy -D warnings; parser-style crates deny unwrap_used/expect_used/indexing_slicing. LICENSE Apache-2.0. deny.toml with the Part 3.3 allow-list. scripts/check-readonly.sh (Part 3.2) + its allow-list file. A `cargo ci` alias (fmt --check, clippy, test, deny, check-readonly). GitHub Actions: macos-latest + ubuntu-latest on main, dev/**, PRs. Branch dev/v1 off main.

B. reclaim-block (doc 03 §2.1)
- BlockSource trait exactly as specified (no write method), ReadResult with per-sector good/bad bitmap, SourceId. Impls: ImageFile (raw), RawDevice (macOS /dev/rdiskN, O_RDONLY, sector-aligned reads, EIO -> Bad sectors not errors), OffsetView, ReadAheadCache (LRU 1 MiB chunks), BadBlockMap type + (de)serialization. FaultInjector wrapper for tests (returns Bad at chosen LBAs). Unit tests + proptest for alignment math.

C. reclaim-platform-macos (doc 06 §1, §7)
- IOKit + DiskArbitration enumeration: BSD name, size, logical/physical block size, whole/leaf, removable, bus, model, serial, content type, mount point, FS kind, APFS container<->volume relations (IOKit classes or `diskutil apfs list -plist` fallback), SMART summary via IOKit where available else None. Verify APIs against current Apple docs; crates io-kit-sys/core-foundation or direct ffi — decide and log.

D. CLI: `reclaim list` (table + --json per doc 07 §2), `reclaim info <SOURCE>` (scheme/size/sector sizes/mounts/health + a 1 s read-speed probe), `reclaim doctor` (root? FDA? SIP status informational; fix-it text). Exit codes doc 07 §3. --prove-readonly logs every open() with flags.

E. Golden-image generator (doc 09 §2) — testdata/recipes/*.toml + scripts/gen-images (Rust or Python): hdiutil create -fs ExFAT|MS-DOS FAT32|APFS|HFS+J, attach, populate from testdata/samples (generate synthetic JPEG/PNG/MP4/TXT of varied sizes — no copyrighted media), delete per recipe, for APFS run 300+ create/delete transactions, detach. Record ground truth (name, hash, extents via fcntl F_LOG2PHYS) to a sidecar JSON. Ship 4 recipes: exfat-camera-delete, fat32-usb-delete, apfs-delete-history, hfsplus-delete. Images are built, never committed (.gitignore).

F. Platform proofs — run on this Mac and WRITE RESULTS INTO doc 06, removing each [verify]: (1) sudo read of /dev/rdiskN for an external sacrificial device and for an attached hdiutil image works; (2) raw read of the boot physical store with/without FDA — if sudo prompts and Ryker is absent, record "needs Ryker" in Manual items instead of stalling; (3) whether reads from the synthesized FileVault-unlocked Data volume device return decrypted APFS blocks (look for NXSB/APSB-looking structures vs high entropy); (4) TRIM timing: delete a 100 MB file on an external SSD vs SD card vs internal, wait 60 s, check whether its extents read zeros. Document device names for the next phases.

Rules: Part 1.4. Never erase or write any disk. Decide routine calls; log them.

Gates: cargo ci green on both runners; check-readonly passes; `reclaim list/info/doctor` run; 4 golden images build locally with ground-truth sidecars; doc 06 has no [verify] left (or each is a Manual item). Commits "p0:" pushed to dev/v1. Write docs/build-log/phase-0.md (Part 1.5) incl. device names, image paths, and the proof results.
```

**Your check:** `sudo reclaim list` shows your internal disk, the APFS container and volumes, and the plugged-in sacrificial card; `reclaim doctor` reports root/FDA status correctly; `ls testdata/build/` shows the four images; read the proof results in `docs/plan/06-…` and the build log.

---

### Phase 1 — Carving engine, imager, sessions, CLI

**Goal:** a PhotoRec-class deep-scan carver with validators, a multi-pass imager with bad-block map, SQLite sessions with pause/resume, `scan --deep / results / recover / image / report`, and a benchmark harness that compares against PhotoRec on the golden images. Tag `v0.1.0`.

**Prompt:**

```
/goal Phase 1 of the Reclaim build: carving engine, imager, sessions, CLI. Read docs/plan/05-file-signature-catalog.md (§1-§4, §6), 03-architecture.md (§2.3, §2.5, §2.6, §3), 07-cli-spec.md, 09-testing-and-validation.md (§3, §4), reclaim-build-guide.md Parts 1.4, 3.2, 3.4, 3.5, and docs/build-log/phase-0.md. Branch dev/v1. Fragment reassembly is round 2 — carve contiguous only and emit Truncated.

A. reclaim-sigs: catalog/*.toml per doc 05 §1 schema; build.rs validates and codegens a table; seed the Tier-1 rows of doc 05 §3 (images, all camera RAW, video, audio, documents, archives, db/mail, executables, disk/crypto, 3D/fonts) — write each magic from the primary spec, fix any doc 05 row you find wrong and note it. `reclaim sigs list|test <file>`.

B. reclaim-carve (doc 03 §2.3): sequential reader -> Aho-Corasick matcher (block-aligned fast path; --brute-force byte path in non-zero, non-uniform blocks) -> validator -> extractor strategies fixed|header_size|footer|structural|statistical -> CarvedFile{offset,len,format,validity Full|Truncated|Suspect,score,embedded name/date}. Block-size inference from GCD of first ~10 validated header offsets when no FS hint. Dedup/priority rules doc 05 §2 (container claims range; RIFF/ZIP/ISOBMFF/TIFF refinement). Validators (doc 05 §4): jpeg, png, gif, bmp, tiff(+RAW Make refinement table), isobmff (mp4/mov/heic/cr3/m4a; mdat-first, largesize, size-0), riff (wav/avi/webp), iff (aiff), zip (+ooxml/iwork/odf/epub/jar refinement, zip64), ole2 (FAT walk), pdf (last %%EOF + startxref sanity), sqlite (exact size), text (IoC + UTF-8/16 classifier), gzip/bzip2/xz/zstd/7z/rar/tar, mach-o/elf/pe, dmg koly (footer-anchored), bplist, mp3/flac/ogg, mkv/ebml, mpeg-ts sync. Metadata extraction for naming (doc 05 §6). cargo fuzz target per validator; seed corpora from testdata/samples.

C. Imager (doc 02 FR-IMG): reclaim-block/src/imaging/{dest.rs,…}: multi-pass (large blocks skip on error; retry bad regions smaller; optional reverse pass), sparse output, optional zstd frames with index, per-chunk + whole BLAKE3/SHA-256, sidecar map, resume. MappedImage source returns Unread for never-read ranges. `reclaim image`, `reclaim verify-image` (doc 07).

D. reclaim-session (doc 03 §2.5, Part 3.5): SQLite schema, deterministic ids, bounded channels, single writer, checkpoint every 5 s/1 GiB, SIGINT -> checkpoint -> exit 4, --resume. Events NDJSON per doc 07 §2. Planner skeleton: chooses engines (only carve exists) and block size.

E. CLI: `scan --deep` with all doc 07 flags that apply, `results` (filters, --format table|json|csv|ids), `recover` (preserve-paths/flat, collisions, --verify, manifest.json, same-whole-disk refusal via platform identity -> exit 5), `report json|html|csv`, `preview <id>` (writes raw extracted bytes / embedded JPEG thumbnail). reclaim-report crate.

F. Benchmark (doc 09 §3): scripts/bench.sh runs reclaim and photorec (brew install testdisk; run it, never read its code) on the four Phase 0 images, computes content_recall/precision/partial_credit/time/peak_rss from the ground-truth sidecars, writes docs/build-log/phase-1/bench.md. Target: content_recall >= photorec on every image; fix validators until true or document why not.

Rules: Part 1.4. No write paths outside dest.rs and reclaim-session (check-readonly enforces). No GPL code. No unwrap on data.

Gates: cargo ci green; every validator has a fuzz target that ran >= 2 min clean; scan/resume test (kill at 40%, resume, identical result set); recover refuses same-disk destination; bench table written and target met or explained. Tag v0.1.0 on dev/v1. Commits "p1:" pushed. Write docs/build-log/phase-1.md incl. the bench table and the list of signatures seeded.
```

**Your check:** `sudo reclaim scan --deep disk4 --session ~/s1` on the sacrificial SD card (after deleting a few test photos from it); watch files stream in; `reclaim results ~/s1 --family image`; `reclaim recover ~/s1 ~/Recovered --verify`; open a recovered JPEG. Read `bench.md` — Reclaim ≥ PhotoRec per image.

---

### Phase 2 — Partition tables + exFAT / FAT / NTFS metadata recovery

**Goal:** the metadata engine with the three camera/Windows filesystems, partition-scheme parsing, allocation bitmaps feeding the carver, merge/de-dup of carved vs named results, and `scan --quick` with paths and timestamps.

**Prompt:**

```
/goal Phase 2 of the Reclaim build: partition tables, exFAT/FAT/NTFS metadata engine, merge. Read docs/plan/04-filesystem-support-matrix.md (§1, §2 rows exFAT/FAT/NTFS/ReFS, §3.3, §3.4), 03-architecture.md (§2.2, §2.5, §5), 09-testing-and-validation.md (§2, §3), reclaim-build-guide.md Parts 1.4, 2.3 (trap 2, 3), 3.6, and docs/build-log/phase-1.md. Branch dev/v1.

A. reclaim-part: GPT (primary + backup header, CRC32, Apple type GUIDs from doc 04 §1), MBR/EBR chains, hybrid MBR, APM detect-only, APFS container and Core Storage detect-only (consumed in Phases 3/4). Output PartitionMap{entries, scheme, confidence}. Planner uses it to build OffsetViews; `reclaim info` prints it. Fuzz targets.

B. reclaim-fs-core: FileSystem trait and Entry/Extent/Bitmap types exactly per doc 03 §2.2 (state Live|Deleted|Orphaned|Historical, confidence, raw name bytes + UTF-8). EntrySink writes to the session. Recoverability score = validator/structure confidence x unallocated-extents fraction x not-overwritten.

C. fs-exfat (doc 04 §3.4): boot sector + checksum, upcase, $BITMAP, directory walk incl. deleted entry sets (0x05/0x40/0x41), NoFatChain handling, zeroed-chain -> assume contiguous + Suspect. fs-fat: FAT12/16/32 BPB + backup boot sector, LFN reconstruction, 0xE5 entries, first-cluster heuristics (search candidate clusters near low-16 value for a matching header on FAT32), both FATs compared. fs-ntfs (doc 04 §3.3): evaluate the permissive `ntfs` crate first (log the decision); need $MFT runs, record walk with in-use flag, $FILE_NAME/$DATA/$STANDARD_INFORMATION, resident data, compressed/sparse runs, $Bitmap, orphan FILE0 records by scanning, $I30 slack names, $UsnJrnl:$J names (best-effort). Each crate: probe() with confidence, allocation_bitmap(), walk(), read_extents(); deny-unwrap lints; fuzz targets for boot sector/dir entry/MFT record parsers.

D. Engine integration: `scan --quick` runs metadata engines; `scan` (default) runs quick then deep in one sequential read where possible (doc 03 §3, §5). Carver consumes the FS bitmap: --unallocated-only, and labels results allocated/unallocated. Merge rule doc 03 §2.3/§5 step 6: a carved range equal to a named entry's extents collapses into it. `results --format tree` prints recovered paths; --deleted-only, --engine filters work.

E. Golden images + scoring: Docker (mkfs.ntfs, mkfs.vfat) recipes: ntfs-delete, ntfs-quick-format, fat16-camera, exfat-partial-overwrite, exfat-reformat-to-fat32, gpt-deleted-partition. Extend scripts/bench.sh with named_recall per doc 09 §3; target named_recall >= 0.95 on the *-delete images, content_recall >= Phase 1 numbers (no regression).

Rules: Part 1.4. Implement from public specs (Microsoft exFAT spec, FAT spec, NTFS docs); no GPL code; no writes; no unwrap on data. Decide crate-vs-own for NTFS and log it.

Gates: cargo ci green on both runners; fuzz targets ran >= 2 min clean; bench table with named_recall column meets targets or is explained; quick+deep merged scan on the exfat images shows no duplicate of a named file. Commits "p2:" pushed. Write docs/build-log/phase-2.md incl. the bench table and any doc 04 corrections.
```

**Your check:** delete three photos on the sacrificial SD card, `sudo reclaim scan disk4 --session ~/s2`, then `reclaim results ~/s2 --deleted-only --format tree` shows `DCIM/…/IMG_xxxx.JPG` with the right names and dates; recover one and compare with the original you copied off earlier.

---

### Phase 3 — APFS + HFS+ metadata recovery, snapshots

**Goal:** the Mac differentiator. APFS checkpoint/snapshot history walking, orphan node scanning, HFS+ catalog slack + journal mining, and `reclaim snapshots`.

**Prompt:**

```
/goal Phase 3 of the Reclaim build: APFS and HFS+ metadata engines, snapshots. Read docs/plan/04-filesystem-support-matrix.md (§2 rows APFS/HFS+, §3.1, §3.2, §4), 06-macos-platform-notes.md (§1, §3, §4, §6), 03-architecture.md (§2.2), reclaim-build-guide.md Parts 1.4, 2.3 (traps 2, 3), 3.6, and docs/build-log/phase-2.md. Branch dev/v1. Implement from Apple File System Reference and TN1150 only (Part 2.3 trap 3).

A. fs-apfs (doc 04 §3.1, steps 1-8 except encryption):
- obj_phys_t + Fletcher-64 verification; nx_superblock_t; checkpoint descriptor/data ring enumeration -> every valid NXSB by xid (the history); omap B-tree (physical, with xid-aware lookup); apfs_superblock_t per volume; FS-tree walk of j_inode/j_drec(+hashed)/j_file_extent/j_xattr (decmpfs, ResourceFork) records; space-manager bitmap for allocation state; snapshots via apfs_snap_metadata tree (sblock_oid per snapshot).
- Diff algorithm: entries present at an older xid/snapshot but absent now -> Deleted/Historical with path from the old tree, extents from old records, score lowered for extents allocated now. Orphan scan: unallocated blocks that checksum as BTREE_NODE/FSTREE -> Orphaned records.
- Volume roles: skip the sealed System volume by default (flag to include); target Data volumes. Encrypted volumes: per Phase 0 proof result, read via the OS-unlocked device or report "locked" — no own crypto (Part 3.6).
- Fuzz: superblock, omap node, fs-tree node, drec parsing.

B. fs-hfs (doc 04 §3.2): volume header + alternate VH, catalog/extents-overflow/attributes B-trees, node slack scan for stale kHFSPlusFileRecord/FolderRecord, journal (jhdr/block list) replay backwards for older catalog nodes, decmpfs/resource forks, hard-link private dir, Unicode name decomposition. Case-sensitive (HX) variant. Fuzz: VH, node, record parsers.

C. CLI + planner: APFS containers from Phase 2's detection -> volumes as sources (`disk3s5` style names from platform-macos). `reclaim snapshots <VOLUME>` lists APFS snapshots + reachable checkpoint xids; `snapshots … diff --from <snap>` lists files present there but not now. Quick scan reports Historical entries with the xid/snapshot they came from. `mounted:/path` POSIX-walk source for Trash/.Trashes (doc 06 §6, §8) — read-only.

D. Golden images: extend gen-images with apfs-snapshots (tmutil-free: `diskutil apfs` snapshot isn't available for images — use fs_snapshot_create via a tiny C/Rust helper or skip and rely on checkpoint history; log the choice), apfs-clone-compress (clonefile + compressed files via ditto --hfsCompression), apfs-many-deletes (2k files, delete half), hfsplus-journal-history, hfsplus-case-sensitive. Bench: named_recall >= 0.90 on apfs-delete-history and apfs-many-deletes (checkpoint depth limits this — report how many xids were recoverable), >= 0.95 on hfsplus-delete.

Rules: Part 1.4. No GPL/LGPL APFS code (read apfs-fuse/libfsapfs only to understand); cite spec sections in doc comments. No writes; no unwrap on data. Never scan the live boot Data volume in tests.

Gates: cargo ci green; fuzz targets ran >= 3 min clean; bench table meets targets or explains checkpoint-depth limits with numbers; `snapshots` works against a real local snapshot (read-only, Ryker-present if sudo needed). Commits "p3:" pushed. Write docs/build-log/phase-3.md incl. bench, recoverable-xid depth observed, and any doc 04 §3.1 corrections.
```

**Your check:** on the sacrificial external SSD formatted APFS, copy a folder, delete it, `sudo reclaim scan disk5s1 --quick --session ~/s3`; the deleted folder and its files should appear as Historical with paths. `sudo reclaim snapshots disk3s5` lists your local Time Machine snapshots.

---

### Phase 4 — Lost volumes, ext family, image containers, health, Recovery-Mode build

**Goal:** damaged and re-partitioned media become recoverable; DMG/VMDK/VHDX/E01 images open as sources; drive health nudges toward image-first; the CLI runs from macOS Recovery.

**Prompt:**

```
/goal Phase 4 of the Reclaim build: lost-structure engine, ext2/3/4 + ISO, image containers, health, Recovery-Mode build. Read docs/plan/04-filesystem-support-matrix.md (§2 rows ext/XFS/Btrfs/F2FS/UFS/ZFS/ISO, §5), 03-architecture.md (§2.4), 06-macos-platform-notes.md (§4, §7, §8), 02-product-requirements.md (FR-SCAN-4, FR-DEV-3), reclaim-build-guide.md Parts 1.4, 3.6, and docs/build-log/phase-3.md. Branch dev/v1.

A. reclaim-structs (doc 03 §2.4): anchor scanners for every row in doc 04 §1-§2 (GPT/MBR headers incl. backup GPT, NXSB/APSB, HFS+ VH + alt VH, NTFS boot + FILE0 density, FAT BPB + backup, EXFAT, ext 0xEF53 + group backups, XFSB, Btrfs magic @64K, ZFS uberblock, UFS, CD001, VMFS). Each hit -> ProposedVolume{start,len,fs,confidence,evidence}; cross-validate (alt header matches size; FS probe succeeds at proposed offset). `reclaim volumes <S>` lists; `volumes <S> adopt N` creates `session:<S>/volume/N` sources usable by scan. Structure pass runs inside the same sequential read as deep scan.

B. fs-ext (doc 04 §2 ext row): superblock + backups, group descriptors, inode tables, dirent walk with slack recovery of deleted names (rec_len absorption), ext2 direct/indirect pointer recovery, ext3/4 jbd2 journal scan for prior inode copies, extent-header 0xF30A scan in unallocated blocks, bitmaps. fs-iso: ISO9660/Joliet/UDF read-only listing. XFS/Btrfs/F2FS/UFS/ZFS: probe() + block size only (detect-and-carve, Part 3.6). Fuzz targets.

C. Image containers (doc 04 §5) in reclaim-block: DMG UDIF (koly + mish; UDRO/UDZO/UDBZ/ULFO/ULMO via zlib/bzip2/lzfse/lzma crates — dynamic-free, permissive), sparseimage, VMDK (flat + sparse), VDI, VHD/VHDX, QCOW2, EnCase E01 (zlib chunks + CRC), split .001 sets. Each as a BlockSource with random access; tests from images generated via hdiutil/qemu-img in CI (qemu-img via brew/apt).

D. Health + robustness (doc 06 §7): SMART via IOKit for SATA/NVMe where exposed; read-latency/error probe fallback for USB; `reclaim info` health verdict; planner recommends image-first when red; `scan` warns. FR-DEV-3 read-error policy (retry N, mark Bad, stride skip). Device-disappeared handling: auto-checkpoint and exit 4 with a resumable session; `--resume` re-identifies the source by SourceId + first/last MiB hashes (doc 07 §4).

E. Recovery-Mode build (doc 06 §4, doc 10 M5): a CLI build with no GUI frameworks, statically linked where macOS allows (C runtime aside), universal; script scripts/recovery-usb.sh that documents/automates putting it on a USB stick; docs/recovery-mode.md with the exact Recovery Terminal steps (csrutil untouched). Test that the binary runs from a plain Terminal with only /usr/lib system libs (otool -L check in CI).

Rules: Part 1.4. No GPL (libewf/libyal read-only for understanding). No writes; no unwrap on data. No new dependencies beyond permissive compression crates.

Gates: cargo ci green; golden images gpt-deleted-partition, exfat-zeroed-boot, hfs-zeroed-vh, ext4-delete, ext2-delete, ntfs-in-vmdk, apfs-in-dmg recover per doc 09 targets; fuzz >= 2 min per new parser; otool -L of the recovery build shows only system libs. Commits "p4:" pushed. Write docs/build-log/phase-4.md incl. container list, health heuristics chosen, and the recovery-mode procedure.
```

**Your check:** zero the first 1 MiB of the sacrificial USB stick's image copy (not the stick) with `dd` on the *image file*, then `reclaim volumes image.img` proposes the FAT32 volume; `adopt 1` and `scan` recover names. `reclaim info disk4` shows a health verdict. Read `docs/recovery-mode.md`.

---

### Phase 5 — SwiftUI app, privileged helper, previews, notarization

**Goal:** Reclaim.app as a thin client over the core: onboarding (helper + FDA), source list, live results with previews, recover sheet, image tool, lost volumes, snapshot browser, reports; signed and (if the account exists) notarized. Tag `v0.5.0-beta`.

**Before you run it (manual):** Part 5.3 (Apple Developer account, Developer ID certificate, `notarytool` keychain profile) if you want a notarized build out of this phase; otherwise the phase produces an ad-hoc build and exact commands.

**Prompt:**

```
/goal Phase 5 of the Reclaim build: SwiftUI app, privileged helper, previews, signing. Read docs/plan/08-gui-spec.md (all), 06-macos-platform-notes.md (§2, §9, §10), 03-architecture.md (§7), 02-product-requirements.md (FR-RES, NFR-7, NFR-8), reclaim-build-guide.md Parts 1.4, 2.3 (traps 1, 4), 3.1, and docs/build-log/phase-4.md. Branch dev/v1. The GUI contains no recovery logic — it calls the core.

A. reclaim-ffi + reclaim-preview
- UniFFI bindings exposing the surface in doc 08 §5 (list_sources, probe, open/start/pause/resume/stop, query paged from SQLite, preview, recover, image, volumes, snapshots, report) with an EventSink callback. Verify current uniffi version/API. Build a Swift package `ReclaimCore` from the generated bindings + universal static lib (aarch64 + x86_64, lipo).
- reclaim-preview: thumbnails/previews from extents without writing to disk — JPEG/PNG/GIF/BMP/WebP via `image`; HEIC and RAW embedded JPEG via the validators' thumb offsets (HEIC full decode can use ImageIO on the Swift side); PDF page 1 and video first frame are done Swift-side with PDFKit/AVFoundation from a temp extraction in the session dir (not the source). Text and hex previews in Rust.

B. Reclaim.app (Xcode project under apps/Reclaim.app, macOS 13+, SwiftUI)
- Screens per doc 08 §2: onboarding (helper install via SMAppService.daemon, FDA check by attempting a boot-store raw read through the helper, deep link to Privacy settings), sidebar sources/tools/sessions, source detail with partition bar + health chip + warnings + Image-first swap when red, scan/results screen (progress per pass, filter rail, grid/list/tree, inspector with preview + metadata + extents/overwrite + validity), recover sheet (same-disk refusal, space check, collision policy, verify), image tool with live bad-sector strip, lost volumes adopt, RAID builder OMITTED (round 2 — leave a disabled menu item), snapshot browser (no root needed), reports HTML/JSON/CSV. States table doc 08 §3 (helper missing, FDA missing, source unplugged -> auto-pause, core panic non-fatal). Keyboard + VoiceOver per §4. System appearance, SF Symbols, no custom chrome.
- Privileged helper (doc 03 §7): XPC API limited to openDeviceReadOnly(bsdName)->fd, smart(bsdName), unlockAPFS(volume, passphrase) via diskutil; fd passed to the in-process core. Helper never parses data. Code requirement checks both ways.

C. Signing/packaging (Part 2.3 trap 4): scripts/build-app.sh builds universal app + embedded helper + LaunchDaemons plist (BundleProgram), hardened runtime entitlements (no sandbox). If DEVELOPMENT_TEAM + Developer ID cert present: codesign, create DMG (create-dmg or hdiutil), notarytool submit --wait, staple. Else ad-hoc sign and print the exact commands for Phase 6. Sparkle is deferred to Phase 6 (needs appcast hosting decision).

D. Tests: XCTest for view models (filters -> SQL, destination validation), UI smoke test launching against a session from a golden image; FFI round-trip tests in Rust; `cargo ci` still green.

Rules: Part 1.4. No network code in the core; no telemetry. No redesign of the CLI contract — the app reads the same session files. Decide and log routine UI calls; do not stop for the Apple account — fall back to ad-hoc.

Gates: cargo ci green; xcodebuild test green; app launches, installs helper, scans a golden image and the sacrificial card, previews thumbnails while scanning, recovers with verify, refuses same-disk destination; ad-hoc or notarized build artifact under dist/. Tag v0.5.0-beta. Commits "p5:" pushed. Write docs/build-log/phase-5.md incl. signing status and the exact notarization commands if skipped.
```

**Your check:** open `dist/Reclaim.app`; helper install prompt appears; grant FDA when asked; scan the sacrificial SD card — thumbnails appear while the scan runs; recover to your Desktop; try to recover onto the card itself (refused). Check VoiceOver reads a grid cell.

---

### Phase 6 — Benchmark, hardware QA, docs, release 1.0.0

**Goal:** publish the numbers, run the hardware checklist with Ryker, finish user docs, sign/notarize, tag `v1.0.0`, publish GitHub Release + Homebrew tap. Code fixes found in QA happen here; no new features.

**Before you run it (manual):** Part 5.3 complete (Developer account, cert, notary profile); Part 5.4 hardware on the desk; decide the Homebrew tap repo name.

**Prompt:**

```
/goal Phase 6 of the Reclaim build: benchmark, hardware QA, docs, release v1.0.0. Read docs/plan/09-testing-and-validation.md (§3, §6, §7), 11-safety-legal-licensing.md, 01-competitive-analysis.md (§2 checklist), reclaim-build-guide.md Parts 1.3, 1.4, 3.7, 5.3-5.4, and all docs/build-log/phase-*.md. Branch dev/v1. Fix bugs found here; add no features.

A. Benchmark publication
- Run scripts/bench.sh across ALL golden images (Phases 0-4) for reclaim and photorec; add the nightly 512 GiB sparse image with 2M files for throughput/peak RSS (doc 09 §7). Write docs/benchmarks.md: methodology (doc 09 §3 verbatim), per-image table (content_recall, named_recall, precision, partial_credit, time, bytes_read, peak_rss, photorec column), environment. Any image where reclaim < photorec on content_recall: fix or explain in a footnote.
- Throughput gate: deep scan >= 80% of `dd bs=4m` on the sacrificial SSD and SD (NFR-2); memory <= 2 GB on the 512 GiB image (NFR-3). Profile and fix if missed.

B. Hardware QA (doc 09 §6) — WITH Ryker present for sudo/FDA/FileVault steps; for each item write PASS/FAIL + evidence into docs/build-log/phase-6/hardware.md:
- Apple Silicon Mac, FileVault on: list/info/quick scan of the Data volume in normal boot; CLI from Recovery Terminal via the Phase 4 USB procedure (Ryker boots it; you prepare the stick and the exact steps).
- Sacrificial SD cards from >= 2 cameras (format in camera, shoot, delete, scan) vs Disk Drill trial + PhotoRec: compare counts and named recovery.
- USB HDD/stick with injected bad sectors (FaultInjector on its image) -> imager completes with map -> scan the image.
- NTFS and ext4 externals (Docker-made sticks OK), Windows-formatted exFAT, FAT32 stick.
- Hot-unplug mid-scan -> resume. Recover 20+ GB with --verify. Same-disk refusal in CLI and app.
- Fresh macOS VM (or second user): download DMG, Gatekeeper passes, helper installs, FDA flow, scan works.

C. Cross-check the plan: walk doc 01 §2 feature inventory and doc 02 FR/NFR ids; write docs/build-log/phase-6/verification.md with PASS / ROUND-2 / N/A per item and evidence. Every round-1 item must be PASS or have a bug fixed in this phase.

D. Docs + legal: README (install, 60-second tutorial, safety promises doc 11 §1, chances-by-media honesty doc 06 §3), docs/cli.md generated from clap (clap_mangen + completions), docs/recovery-mode.md final, CONTRIBUTING.md (signature PR rules Part 3.4, DCO), SECURITY.md, THIRD_PARTY.md via cargo about, EULA/first-run text per doc 11 §5, CHANGELOG.md [1.0.0]. Bump all versions to 1.0.0.

E. Release: scripts/release.sh builds CLI tarballs (aarch64/x86_64/universal darwin, x86_64 linux), universal app, codesign + notarize + staple the DMG and the CLI (Developer ID), SHA256SUMS. Sparkle: add framework + appcast.xml served from GitHub Releases; sign with EdDSA key Ryker generates (Part 5.3). Open PR dev/v1 -> main; when green, merge, tag v1.0.0, `gh release create` with artifacts + benchmark summary. Create homebrew-reclaim tap repo with formula (CLI, sha256) + cask (DMG); `brew install --build-from-source` test passes.

Rules: Part 1.4. No writes to any non-sacrificial disk. Do not publish until Ryker says ship (the PR can wait). Never soften the benchmark — publish the real numbers.

Gates: cargo ci green on main; hardware.md and verification.md complete with no unexplained FAIL; notarization stapled (spctl --assess passes); v1.0.0 tagged; GitHub Release live; `brew install <tap>/reclaim` and the cask install on a clean machine. Write docs/build-log/phase-6.md incl. release URLs, checksums, and the round-2 backlog (fragment reassembly, RAID, crypto, ext polish, Windows/Linux GUI).
```

**Your check:** on a second Mac (or fresh user), `brew install <tap>/reclaim`, `brew install --cask reclaim`, open the app, scan an SD card. Read `docs/benchmarks.md` and post the table where you like.

---

## Part 5 — Manual blockers (everything that needs you)

Nothing in Part 4 stops for a question except `sudo`, FDA, and FileVault prompts, which only a human can answer. Do these in this order; 5.2–5.4 can run in parallel with early phases.

### 5.1 Before Phase 0 (30 minutes)
- [ ] Create the empty GitHub repo (`reclaim`, default branch `main`), clone to the Mac, copy the 13 planning docs into `docs/plan/` and this guide to `reclaim-build-guide.md` at the repo root, commit, push.
- [ ] Toolchain: Xcode + command-line tools, `rustup` (stable), `brew install testdisk qemu create-dmg cargo-deny cargo-about cargo-fuzz` (cargo-fuzz needs nightly: `rustup toolchain install nightly`), Docker Desktop or OrbStack (for `mkfs.ntfs`/`mkfs.ext4` images).
- [ ] Grant **Full Disk Access** to the terminal app you run Claude Code from (System Settings → Privacy & Security → Full Disk Access) and run `sudo -v` right before starting Phase 0 so the agent's `sudo` calls don't prompt mid-run. Be present for Phase 0 step F; re-run `sudo -v` if it times out.
- [ ] Confirm `gh auth status` works (PR/release creation).

### 5.2 Sacrificial test media (before Phase 1; keep forever)
Label each "RECLAIM TEST — ERASE OK". Nothing else is ever scanned with `sudo` during development.
- [ ] Two SD cards (≥ 32 GB) + a reader — format in a real camera, shoot real throwaway photos/video, delete some; these are the camera corpus.
- [ ] One USB stick (FAT32) and one small USB HDD or SSD (will be APFS, then HFS+, then exFAT across phases).
- [ ] Optional but valuable: an old failing HDD for the imager (Part 4 / 6 hardware checklist).
- [ ] Write the BSD names (`diskutil list`) into Part 5.2 of this file once plugged in, so the agent's build logs match.

### 5.3 Apple Developer & release accounts (before Phase 5 for a notarized beta; mandatory before Phase 6)
- [ ] Apple Developer Program membership; create a **Developer ID Application** certificate and install it in the login keychain; note the Team ID.
- [ ] `xcrun notarytool store-credentials reclaim-notary --apple-id … --team-id … --password <app-specific>`.
- [ ] Export `DEVELOPMENT_TEAM=<TeamID>` in the shell profile the agent inherits.
- [ ] Generate the Sparkle EdDSA key pair (`generate_keys` from the Sparkle release); keep the private key out of the repo; give the public key path to Phase 6 via the build log Manual items.
- [ ] Decide the Homebrew tap name (default `homebrew-reclaim` under your GitHub account) and create the empty repo.

### 5.4 Hardware for Phase 6 QA
- [ ] Your Apple Silicon Mac with FileVault on (it already is, most likely) — be present for the boot-disk and Recovery-Mode steps; back up first (Time Machine) because you will boot from a USB stick.
- [ ] Ideally one Intel Mac (T2 or not) for the second column of doc 09 §6; skip if unavailable and mark N/A.
- [ ] A second Mac, or a fresh macOS user account, for the clean-install Gatekeeper test.

### 5.5 Decisions only you can make (answer in the build-log Manual items when asked)
- [ ] **Name.** "Reclaim" is a placeholder; check the App/brand namespace you want and rename before Phase 5 (bundle id, helper label, tap name). A rename later touches signing identities.
- [ ] **License line.** Apache-2.0 core is locked (Part 3.3). Decide before Phase 6 whether the GUI stays open (single LICENSE) or goes open-core (doc 01 §4 option B) — it changes CONTRIBUTING.md and the release notes.
- [ ] **Publish benchmarks** comparing against PhotoRec and Disk Drill by name, or anonymized. The guide assumes by name with methodology attached (doc 11 §3).

### 5.6 Ongoing
- [ ] Every new camera, card, or weird drive you meet → a golden-image recipe or a private corpus image (hashes only in the repo).
- [ ] Triage signature PRs: sample file + TOML + a test; reject anything copied from PhotoRec's source.

---

## Part 6 — Definition of done for round 1

Round 1 is finished when all of these are true:

- `v1.0.0` tagged on `main`; GitHub Release carries a notarized, stapled `Reclaim-1.0.0.dmg`, CLI tarballs for arm64/x86_64/universal macOS and x86_64 Linux, and `SHA256SUMS`; `brew install <tap>/reclaim` and the cask work on a clean Mac.
- `cargo ci` is green on macOS and Linux runners, `check-readonly` passes, `cargo deny` passes, every parser and validator has a fuzz target that has run clean, and every integration test proves the source image hash is unchanged.
- `docs/benchmarks.md` shows Reclaim ≥ PhotoRec on `content_recall` for every golden image, `named_recall` ≥ 0.95 on exFAT/FAT/NTFS/HFS+ delete images and ≥ 0.90 on APFS (with recoverable-checkpoint depth reported), precision ≥ 0.95, deep-scan throughput ≥ 80 % of raw device read speed, peak RSS ≤ 2 GB on the 512 GiB image.
- The CLI implements doc 07: `list, info, image, verify-image, scan (quick/deep/default), results, preview, recover, volumes, snapshots, report, sigs, doctor`, NDJSON events, exit codes, resumable sessions, same-disk refusal.
- The app implements doc 08 round-1 screens: onboarding with helper + FDA, live results with previews during scan, recover sheet, image tool with bad-sector map, lost volumes, snapshot browser, reports; VoiceOver-navigable; no custom chrome.
- Filesystem coverage: metadata recovery for APFS (checkpoints, snapshots, orphans), HFS+ (slack, journal), NTFS, exFAT, FAT12/16/32, ext2/3/4 (Tier-2 depth), ISO/UDF listing; detection + carving for XFS, Btrfs, F2FS, UFS, ZFS, ReFS; GPT/MBR/APM/hybrid; sources from raw devices, raw images, DMG (all UDIF compressions), sparseimage, VMDK, VDI, VHD/VHDX, QCOW2, E01, split sets.
- Signature catalog ≥ 150 formats with the doc 05 §4 validators; camera RAW complete.
- `docs/plan/06-macos-platform-notes.md` contains no `[verify]`; `docs/recovery-mode.md` has been executed once on real hardware; `hardware.md` and `verification.md` have no unexplained FAIL.
- Round-2 backlog written: fragment reassembly (JPEG, MP4/MOV), RAID, BitLocker/LUKS/FileVault own-unlock, ext/XFS/Btrfs depth, deletion-journal agent, Windows/Linux GUI.
