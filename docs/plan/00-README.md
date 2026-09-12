# Reclaim — Build Documentation Set

> Working name: **Reclaim** (placeholder). A macOS-first disk, file and photo recovery tool intended to match or exceed Disk Drill / PhotoRec / R-Studio on breadth of filesystem and file-type support.

This folder is the complete planning package for building the tool. Read the documents in order the first time; afterwards they are reference material.

| # | Document | What it answers |
|---|----------|-----------------|
| 01 | [Competitive Analysis](01-competitive-analysis.md) | What Disk Drill, PhotoRec/TestDisk, Recuva, R-Studio, UFS Explorer, Stellar, EaseUS etc. actually do, what they charge, where they are weak |
| 02 | [Product Requirements (PRD)](02-product-requirements.md) | Who it's for, what it must do, what it must never do, success criteria |
| 03 | [Architecture](03-architecture.md) | Layered design, core data model, crate/module layout, concurrency, language choice |
| 04 | [Filesystem Support Matrix](04-filesystem-support-matrix.md) | Every filesystem, partition scheme, container and volume manager in scope, with tier, recovery method and the on-disk structures you need to parse |
| 05 | [File Signature Catalog](05-file-signature-catalog.md) | The carving engine: signature format, validation strategy, and a seed catalog of ~150 formats grouped by family, with headers/footers |
| 06 | [macOS Platform Notes](06-macos-platform-notes.md) | Raw device access, root/Full Disk Access, SIP, T2 & Apple Silicon encryption, FileVault, APFS snapshots, Recovery Mode, notarization |
| 07 | [CLI Specification](07-cli-spec.md) | Command grammar, output formats, exit codes, session files, scripting contract |
| 08 | [GUI Specification](08-gui-spec.md) | SwiftUI shell over the Rust core: screens, flows, states, accessibility |
| 09 | [Testing & Validation](09-testing-and-validation.md) | Corpus generation, golden images, fuzzing, benchmark methodology, recovery-rate scoring |
| 10 | [Roadmap](10-roadmap.md) | Milestones M0–M7 from "can list disks" to "GUI + RAID + video reassembly" |
| 11 | [Safety, Legal & Licensing](11-safety-legal-licensing.md) | Read-only guarantees, destination-disk rules, GPL contamination risks, forensic soundness, distribution |
| 12 | [Glossary](12-glossary.md) | Terms used throughout |

## One-paragraph summary of the plan

Build a **read-only, Rust-based recovery core** exposed first as a **CLI** (`reclaim`), then wrapped in a native **SwiftUI GUI**. The core has three recovery engines that share one block-device abstraction: (1) **metadata recovery** that parses partition tables and filesystems to find deleted-but-unallocated entries with original names and paths; (2) **signature carving** that scans raw blocks for file headers/footers, with per-format validators and fragment reassembly for the formats that matter most on a Mac (JPEG/HEIC/RAW photos, MP4/MOV video); and (3) **structure recovery** that finds lost partitions and volumes by hunting for boot sectors, superblocks and APFS checkpoints. A **byte-to-byte imager** with bad-sector mapping sits underneath so every scan can run against an image instead of a failing drive. Everything is session-based (pause/resume/save) and streams results so previews appear while the scan is still running.

## Non-negotiables (repeated throughout)

1. **Never write to a source device.** Opened `O_RDONLY`; no code path exists that can write to a scanned device.
2. **Never recover onto the source device** without an explicit, loudly warned override.
3. **Image first when the drive is unhealthy** (S.M.A.R.T. warnings, read errors) — the imager is not a bonus feature, it is the entry point.
4. **Deterministic, resumable scans.** A scan interrupted at 40 % resumes at 40 %.
5. **Preview before commit.** Users see thumbnails / text before paying (if commercial) or before writing anything.
