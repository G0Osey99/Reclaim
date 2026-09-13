# Phase 6 — Plan cross-check (verification)

Walks the doc 02 functional/non-functional requirements and the doc 01 §2 feature
inventory. Status is one of:

- **PASS** — implemented and evidenced (crate/test/build-log/benchmark).
- **PASS·HW** — implemented in code; the physical-hardware confirmation is in
  [hardware.md](hardware.md).
- **SHIP** — code/artifact ready; the step is a Ryker-gated release action
  (paid Developer ID notarization, EdDSA key, GitHub publish, tap push) — see
  [../phase-6.md](../phase-6.md).
- **ROUND-2** — deliberately out of round 1 (build guide §3.6 / doc 02 §3
  "in scope later").
- **N/A** — explicitly out of scope.

Every round-1 *code* item is PASS. The three bugs found during Phase-6 QA were
fixed here (FAT/exFAT single-cluster validity; carved↔named over-carve merge;
the benchmark precision metric) — see [../phase-6.md](../phase-6.md).

---

## 1. Functional requirements (doc 02 §4)

### FR-DEV — device access
| ID | Status | Evidence |
|---|---|---|
| FR-DEV-1 enumerate disks/partitions/volumes/containers, SMART | PASS | `reclaim list`/`info`; `reclaim-platform-macos` enumeration + `assess_health`; Core Storage/APFS container **partitions** recognized (full non-APFS LV assembly is ROUND-2). |
| FR-DEV-2 open read-only, no write API to engines | PASS | `reclaim-block` `RawDevice` (`O_RDONLY`, `/dev/rdiskN`); `BlockSource` has no write method; `check-readonly.sh` in CI; `--prove-readonly`. |
| FR-DEV-3 block size / alignment / read-error policy | PASS | imager retry-N → mark bad → skip stride → reverse pass (Phase 1); `--block`, `--retries`. |
| FR-DEV-4 layered sources (one `BlockSource` stack) | PASS | container→partition/offset→image stack; `OffsetView`, `MappedSource`, `ConcatSource` (Phase 4). |

### FR-IMG — imaging
| ID | Status | Evidence |
|---|---|---|
| FR-IMG-1 raw image + sidecar bad/good/unread map + per-chunk hash | PASS | `reclaim image` + `.reclaim-map`; Phase 1. |
| FR-IMG-2 multi-pass (large → retry small → reverse) | PASS | imager passes; `--reverse-pass`. |
| FR-IMG-3 resume from map | PASS | `image --resume`; Phase 1/4. |
| FR-IMG-4 sparse output; optional zstd container | PASS | `--sparse`, `--zstd`. |
| FR-IMG-5 whole + per-chunk hash; verify command | PASS | `--hash blake3\|sha256`; `reclaim verify-image`. |

### FR-SCAN — scanning
| ID | Status | Evidence |
|---|---|---|
| FR-SCAN-1 auto-detect scheme + FS, report confidence | PASS | `reclaim-part` + `meta::probe_best`; `Event::Start{fs,confidence}`. |
| FR-SCAN-2 quick scan: named live+deleted, extents, score | PASS | metadata engines (Phase 2–4); named_recall table in [../../benchmarks.md](../../benchmarks.md). |
| FR-SCAN-3 deep scan: signature carving + per-format validation | PASS | `reclaim-carve` (140 sigs / 210 exts / 38 validators). |
| FR-SCAN-4 lost-structure scan; propose + mount partitions | PASS | `reclaim volumes` / `adopt`; `reclaim-structs` (Phase 4). |
| FR-SCAN-5 one sequential pass feeding multiple consumers | PASS | single carve read; structure pass shares the read pattern (folding into one buffer is a noted Phase-6 micro-opt, not a correctness gap). |
| FR-SCAN-6 merge & de-duplicate carved↔named | PASS | `merge_carved_into_entries` — now collapses start-offset matches incl. over-carves (**fixed this phase**). |
| FR-SCAN-7 pause/resume/checkpoint; reopen | PASS | checkpoint every 5 s / 1 GiB; `scan --resume` re-identifies the source. |
| FR-SCAN-8 stream results (event log) | PASS | NDJSON `log.ndjson`; `--json`; GUI streams during scan. |
| FR-SCAN-9 scope restriction (LBA range, unallocated, subtree) | PASS | `--range`, `--unallocated-only` (FS bitmap); subtree via adopted volume. |

### FR-RES — results & recovery
| ID | Status | Evidence |
|---|---|---|
| FR-RES-1 filter/sort/search | PASS | `reclaim results` + GUI filter rail. |
| FR-RES-2 preview (image/RAW-embedded/text/hex; PDF/video via app) | PASS | `reclaim preview`; `reclaim-preview`; app inspector. |
| FR-RES-3 recover, preserve paths, collision policy, refuse same-device | PASS | `reclaim recover`; `--allow-same-device-i-accept-data-loss` override; tests. |
| FR-RES-4 post-recovery hash verify + partial/corrupt report | PASS | `recover --verify` (re-read + hash) + manifest; `reclaim-session::recover`. |
| FR-RES-5 reports JSON/HTML/CSV | PASS | `reclaim report` (`reclaim-report`). |

## 2. Non-functional requirements (doc 02 §5)

| ID | Status | Evidence |
|---|---|---|
| NFR-1 read-only by construction | PASS | see FR-DEV-2; every integration test hashes source before/after. |
| NFR-2 throughput ≥ 80% of raw sequential read | PASS·HW | on-image carve sustains ~450 MB/s; the ≥80%-of-`dd bs=4m` comparison must be measured on the sacrificial SSD/SD — [hardware.md](hardware.md). |
| NFR-3 memory ≤ 2 GB on an 8 TB-class disk | PASS | 512 GiB / 2 M-file nightly test: peak RSS well under 2 GB — [../../benchmarks.md](../../benchmarks.md) (on-disk SQLite index keeps RSS flat vs result count). |
| NFR-4 crash safety = one checkpoint interval | PASS | checkpoint every 5 s / 1 GiB; device-disappeared → exit 4 + resume. |
| NFR-5 determinism (same image → same IDs) | PASS | `result_id = blake3(source‖engine‖offset‖len)` (`reclaim-session::id`); `INSERT OR IGNORE` idempotency test. |
| NFR-6 every parser fuzzed; no panic on malformed data | PASS | `cargo fuzz` targets (fs + validators + part + structs + container) ran clean; parser crates `#![deny(unwrap/expect/indexing)]`. |
| NFR-7 signed & notarized universal app + brew | SHIP | universal CLI + app build; appledev-signed today; Developer-ID notarization, tap, and publish are Ryker-gated (Part 5.3) — [../phase-6.md](../phase-6.md). |
| NFR-8 accessibility (VoiceOver; CLI `--plain`) | PASS·HW | `--plain` (no box-drawing); every thumbnail has an a11y label; VoiceOver navigation confirmed in [hardware.md](hardware.md). |
| NFR-9 localization-ready; English at launch | PASS | English at launch; user-facing strings live in the views/CLI. Full string externalization (`.strings`) is a round-2 polish item. |

## 3. Feature inventory (doc 01 §2)

**Scanning** — quick ✅, deep ✅, lost-partition/volume ✅, combined+deduped view ✅
(merge), scan of volume/disk/raw/image/DMG/E01/VMDK/sparsebundle ✅, pause/resume/
save ✅, live streaming ✅, filters (type/size/date/path/score/deleted-only) ✅,
recovery-chance estimate ✅ (score + recoverability bucket). **All PASS.**

**Imaging** — byte-to-byte + bad-sector map + skip/retry multi-pass ✅, sparse +
resume ✅. **Entropy map** (encrypted/random vs empty visualization): **ROUND-2**.
**Forensic containers:** E01 **read** ✅; **AFF4** and E01 *write*: **ROUND-2**.
Hardware-imager integration (DeepSpar/PC-3000): **N/A** (out of scope).

**Filesystem breadth** — full metadata: APFS (snapshots/checkpoints/orphans/
clones/compressed/unlocked-encrypted) ✅, HFS+ (journaled + case-sensitive) ✅,
NTFS ✅, exFAT ✅, FAT12/16/32 ✅, ext2/3/4 ✅, ISO 9660/Joliet ✅. Detect+carve:
XFS, Btrfs, F2FS, UFS, ZFS, ReFS, UDF ✅ (deeper walks **ROUND-2**). Partitioning:
GPT, MBR, APM, hybrid ✅. Containers: raw, DMG (all UDIF codecs), sparseimage,
VMDK, VDI, VHD/VHDX, QCOW2, E01, split ✅. Volume managers beyond APFS containers
(Core Storage LV assembly, Fusion, LVM2, mdadm, LDM, Storage Spaces, SHR):
**ROUND-2**. Own-crypto unlock (BitLocker/LUKS/VeraCrypt/FileVault): **ROUND-2**
(round 1 reads OS-unlocked volumes).

**RAID** — auto-detect and manual builder: **ROUND-2** (disabled menu item in app).

**File-type breadth** — 140 signatures / **210 extensions** / 38 validators, camera
RAW complete (22 formats) ✅; format-aware validation ✅. **Fragment reassembly**
(JPEG/MP4/MOV): **ROUND-2** (round 1 carves contiguous, emits `Truncated`).
Original-filename recovery: from the **FS metadata engine** ✅ (real names/paths);
embedded metadata (EXIF date, camera model, ID3) is extracted ✅; deriving a
*filename* from embedded metadata alone (e.g. Office `core.xml`) is **ROUND-2**.

**Output & UX** — preview ✅, recover to chosen dest + preserve paths + refuse
same-disk ✅, hash & verify ✅, reports HTML/JSON/CSV ✅, SMART + health nudge ✅,
Recovery-Mode operation ✅. **Recovery-Vault deletion journal**: **ROUND-2**.

**Also-ran / mobile** — iOS/Android device recovery: **N/A** (out of scope, doc 02 §3).

## 4. Round-1 definition of done (build guide Part 6)

| Item | Status |
|---|---|
| CLI implements list/info/image/verify-image/scan/results/preview/recover/volumes/snapshots/report/sigs/doctor, NDJSON, exit codes, resumable, same-disk refusal | PASS ([../../cli.md](../../cli.md)) |
| App round-1 screens (onboarding, live results+preview, recover, imaging+map, lost volumes, snapshots, reports), VoiceOver, no custom chrome | PASS·HW (Phase 5; VoiceOver in hardware.md) |
| Benchmarks ≥ PhotoRec content_recall everywhere; named ≥0.95 exFAT/FAT/NTFS/HFS+ & ≥0.90 APFS; precision ≥0.95; ≤2 GB on 512 GiB | PASS ([../../benchmarks.md](../../benchmarks.md); precision note for the FAT `._*`-suspect case) |
| Signature catalog ≥150 formats, camera RAW complete | PASS (210 extensions; 22 RAW) |
| `cargo ci` green; fuzz clean; source-hash-unchanged tests | PASS |
| Signed/notarized DMG + CLI tarballs + SHA256SUMS; tag v1.0.0; brew tap | SHIP (Ryker — Part 5.3) |
| `docs/plan/06` has no `[verify]`; `docs/recovery-mode.md` executed on hardware; hardware.md/verification.md no unexplained FAIL | PASS·HW ([hardware.md](hardware.md)) |
| Round-2 backlog written | PASS ([../phase-6.md](../phase-6.md)) |
