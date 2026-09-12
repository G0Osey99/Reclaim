# 02 — Product Requirements Document (PRD)

## 1. Vision

A recovery tool for macOS that a photographer, a developer, and a forensic technician would all reach for: as broad as UFS Explorer, as safe as PhotoRec, as approachable as Disk Drill, and scriptable from day one.

## 2. Target users & jobs-to-be-done

| Persona | Situation | What they need |
|---------|-----------|----------------|
| **Photographer / videographer** (primary) | Formatted an SD card, deleted a shoot, corrupted card in camera | Get every JPEG/HEIC/RAW/MP4/MOV back, with thumbnails, fast; fragmented video reassembled |
| **Everyday Mac user** | Emptied Trash, external drive "not readable", accidental erase in Disk Utility | Guided flow; recover with original names and folders where possible; clear "chances" indicator |
| **Developer / power user** | Wiped the wrong partition, lost a Linux VM disk, wants to script recovery over 20 cards | CLI with JSON output, supports ext4/Btrfs/XFS, images and VM disks, exit codes |
| **IT / repair tech** | Client drive with bad sectors, RAID NAS failed | Image first with bad-sector map, RAID reconstruction, reports for the client |
| **DFIR / forensic analyst** (secondary) | Needs reproducible, read-only acquisition and carving | Hashes, E01 support, deterministic output, audit log |

## 3. Scope

### In scope (v1.0)
- macOS 13+ on Apple Silicon and Intel (Rust core is portable; Linux CLI builds are a cheap bonus and make CI easier)
- Sources: physical disks, partitions/volumes, raw image files, DMG (uncompressed/UDIF read-only), VMDK/VDI/VHD(X) flat & sparse, E01
- Three engines: metadata recovery, signature carving, lost-structure search
- Imager with bad-sector handling and resume
- Filesystems Tier 1 & 2 (doc 04); file signatures Tier 1 & 2 (doc 05)
- CLI (`reclaim`) + SwiftUI GUI
- Session save/resume; preview; recovery with path preservation; HTML/JSON reports

### In scope later (v1.x–2.0)
- RAID 0/1/5/6/10 auto-detection & virtual assembly
- Fragment reassembly for MP4/MOV; JPEG (v1 carves contiguous only)
- Encrypted containers with user-supplied keys (BitLocker, LUKS)
- Filesystem repair (boot-sector/superblock rewrite) — separate, write-capable mode with its own safety gates
- "Deletion journal" background agent (Recovery Vault equivalent)
- Windows and Linux GUI

### Out of scope
- iOS/Android device recovery
- Chip-off / hardware-level (PC-3000-class) work
- Cloud-storage recovery
- Anti-forensics / secure erase

## 4. Functional requirements

IDs are referenced from the roadmap and test plan.

### FR-DEV — Device access
- FR-DEV-1: Enumerate physical disks, partitions, APFS containers/volumes, Core Storage LVs, mounted images with size, model, serial, bus, sector size, mount status, S.M.A.R.T. summary.
- FR-DEV-2: Open any source read-only (`O_RDONLY`; on macOS `/dev/rdiskN` raw character device for unbuffered I/O). Compile-time guarantee that the device layer exposes no write API to engines.
- FR-DEV-3: Configurable read block size, alignment to physical sector; read-error policy (retry N times, then mark bad, then skip forward by configurable stride).
- FR-DEV-4: Layered sources: an engine reads through a stack (RAID → decryption → container/partition offset → image file/device) via one `BlockSource` trait.

### FR-IMG — Imaging
- FR-IMG-1: Create raw image + sidecar map (`.reclaim-map` JSON/binary: good/bad/unread ranges, hashes per chunk).
- FR-IMG-2: Multi-pass strategy: pass 1 large blocks skipping errors; pass 2 retry bad regions with smaller blocks; pass 3 reverse read. Configurable.
- FR-IMG-3: Resume after interruption from the map.
- FR-IMG-4: Sparse output when destination FS supports it; optional compression (zstd) container with random-access index.
- FR-IMG-5: Optional SHA-256 of whole image and per-chunk; verify command.

### FR-SCAN — Scanning
- FR-SCAN-1: Auto-detect partition scheme and filesystems; report confidence.
- FR-SCAN-2: Quick scan: enumerate live + deleted entries with name, path, size, timestamps, extents, recoverability score.
- FR-SCAN-3: Deep scan: carve by signatures with per-format validation, respecting FS block size when known.
- FR-SCAN-4: Lost-structure scan: find boot sectors, superblocks, APFS checkpoints/volume superblocks, GPT backups; propose partitions; allow "mount" of a proposed partition for further scanning.
- FR-SCAN-5: Run engines in one pass over the media where possible (single sequential read feeding multiple consumers) — I/O is the bottleneck on failing drives.
- FR-SCAN-6: Merge & de-duplicate results (carved file whose extent equals a metadata-recovered file collapses into the metadata result).
- FR-SCAN-7: Pause, resume, checkpoint session to disk every N seconds; reopen a session file with the same source.
- FR-SCAN-8: Stream results (event log) so UIs show files as found.
- FR-SCAN-9: Scan scope restriction: LBA ranges, only unallocated space, only a subtree.

### FR-RES — Results & recovery
- FR-RES-1: Filter/sort/search by type, family, size, date, path, score, engine.
- FR-RES-2: Preview: images (incl. RAW via embedded JPEG), video first frames, PDF first page, text, audio waveform/playback, hex.
- FR-RES-3: Recover selected items to destination; preserve paths; name-collision policy; refuse same-device destination unless `--i-understand-overwrite-risk`.
- FR-RES-4: Post-recovery verification: hash, optional format validation, report of partial/corrupt files.
- FR-RES-5: Reports: JSON (machine), HTML (human), CSV.

### FR-CLI / FR-GUI — see docs 07 and 08.

## 5. Non-functional requirements

| ID | Requirement |
|----|-------------|
| NFR-1 | **Read-only by construction**: no `write`/`ioctl` write paths in the device crate; enforced by review + `#![deny]` lint on `OpenOptions::write`. |
| NFR-2 | Throughput: deep scan sustains ≥ 80 % of raw device sequential read speed on healthy media (target ≥ 1.5 GB/s on internal NVMe, ≥ 150 MB/s on USB-3 HDD). |
| NFR-3 | Memory: bounded regardless of media size (result index on disk via embedded KV store, e.g. `redb`/`sled`/`sqlite`); ≤ 2 GB RSS scanning a 8 TB disk. |
| NFR-4 | Crash safety: session checkpoint means at most N seconds of scan lost. |
| NFR-5 | Determinism: same input image → identical result set and IDs (for testing and forensics). |
| NFR-6 | Robustness: every parser fuzzed; malformed metadata never panics (Rust: no `unwrap` on parsed data; `#![deny(clippy::unwrap_used)]` in parser crates). |
| NFR-7 | Distribution: signed & notarized `.app` and `.pkg`; Homebrew formula for CLI; universal binary. |
| NFR-8 | Accessibility: GUI VoiceOver-navigable; CLI output readable by screen readers (no box-drawing in `--plain`). |
| NFR-9 | Localization-ready (strings externalized); English at launch. |

## 6. Success metrics

- Recovery-rate benchmark (doc 09) ≥ PhotoRec on carving corpus; ≥ 95 % of deleted files named correctly on APFS/HFS+/NTFS/exFAT/FAT quick-scan corpus.
- Zero writes to source across the full test suite (verified by hashing source images before/after every test).
- Deep scan of a 64 GB SD card in < 4 min on USB-3 reader.
- False-positive rate of carved files < 5 % on the mixed corpus (validated formats).

## 7. Key decisions (with rationale)

| Decision | Choice | Why |
|----------|--------|-----|
| Core language | **Rust** | Parsing hostile on-disk structures at high speed; memory safety without GC pauses; excellent FFI to Swift; cross-platform CLI for free |
| GUI | **SwiftUI app** calling the Rust core through a C ABI / UniFFI | Native look, notarization, sandbox exemptions handled the Apple way; avoids Electron bloat on a tool that runs next to a dying disk |
| CLI first | Yes | Engines are testable headlessly; GUI is a thin client; forensic users need it |
| Result storage | Embedded on-disk store (SQLite via `rusqlite` recommended) | Millions of carved entries; queryable; session file = one DB |
| Privileges | Helper tool via `SMAppService` (GUI) / `sudo` (CLI) | Raw device reads require root on macOS |
| Licensing | Apache-2.0/MIT core, no GPL code copied | Keeps commercial options open (doc 11) |
