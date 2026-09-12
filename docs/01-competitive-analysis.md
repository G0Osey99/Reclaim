# 01 — Competitive Analysis

Researched September 2026. Prices and version numbers drift; treat those as approximate. Everything else here is what matters for design decisions.

## 1. The landscape in one table

| Tool | Platform | License / price | Core approach | Filesystems | Standout features | Weak spots |
|------|----------|-----------------|---------------|-------------|-------------------|------------|
| **Disk Drill 6** (CleverFiles) | macOS 10.15+, Windows 10+ | Freemium. Mac free tier = preview only (+ "Guaranteed Recovery" protected files); Pro ≈ $89 | Quick scan (metadata) + Deep scan (carving) + lost-partition search | APFS, HFS+, NTFS, exFAT, FAT32, ext4, Btrfs, ReFS; BitLocker on Mac build | Byte-to-byte backup, Recovery Vault (tracks deletions), fragmented video reconstruction (GoPro/DJI/Canon/Insta360), 400+ signatures, live preview during scan, S.M.A.R.T., iOS device recovery, Apple-notarized | Deep-scan results rank below R-Studio/Stellar in third-party tests; Mac free tier recovers nothing |
| **PhotoRec / TestDisk** (CGSecurity) | Everything | GPLv2+, free | Pure signature carving (PhotoRec); partition/boot-sector repair + limited undelete (TestDisk) | Carving is FS-agnostic; uses FAT/NTFS/ext superblock only to learn block size. TestDisk undelete: FAT, exFAT, ext2, NTFS | ~480 extensions / ~300 file families; statistically detects text (index of coincidence); libjpeg-validated JPEGs; runs anywhere incl. from a live USB | No filenames or paths, no APFS/HFS+ metadata recovery, ncurses UI, no preview, no imaging beyond simple `dd`-style copies, ReiserFS tail-packing not handled |
| **Recuva** (Piriform/CCleaner) | Windows only | Free / Pro ≈ $25 | MFT/FAT-table undelete + deep scan (carving) | NTFS, FAT, exFAT | Secure-overwrite of found files, wizard UI, tiny footprint | Windows only; weak on formatted/re-partitioned drives; development slow; no imaging |
| **R-Studio** (R-Tools) | macOS, Windows, Linux | ≈ $80 (Mac); Technician tiers higher | Metadata + carving + RAID reconstruction + hex editor | "Almost everything": APFS, HFS+, NTFS, ReFS, FAT/exFAT, ext2/3/4, XFS, UFS, Btrfs, ZFS... | Best independent deep-scan results; RAID 0/1/5/6 + custom layouts; pause/resume; disk imaging with bad-sector handling; network recovery; bootable media | Interface is engineering-grade and intimidating; pricey for consumers |
| **UFS Explorer / Recovery Explorer** (SysDev) | macOS, Windows, Linux | ≈ €60 – €700+ by edition | Metadata-first with carving; the most complete FS list on the market | FAT/exFAT/NTFS/ReFS, APFS/HFS+/HFS, ext2-4/XFS/JFS/ReiserFS/Btrfs/F2FS, UFS/UFS2/ZFS, VMFS, NWFS/NSS, ISO9660/UDF | RAID 0/1/1E/3/5/6/7 + nested; mdadm, LVM, LDM, Storage Spaces, Apple RAID/Fusion/Core Storage, Synology SHR, Drobo; LUKS/FileVault 2/BitLocker/VeraCrypt; multi-pass imaging with entropy + bad-sector maps; E01/AFF4 forensic images; sector-size mutation (520/528 B); hex editor with structure highlighting | Complexity, price ladder, dated UI |
| **Stellar Data Recovery for Mac** | macOS | ≈ $80/yr or $99 lifetime | Metadata + carving | APFS, HFS+, NTFS, exFAT, FAT | Disk imaging, bootable recovery drive, S.M.A.R.T., strong recovery results in tests | Preview only after scan completes; pause/resume reported buggy; huge files sometimes truncated |
| **EaseUS Data Recovery Wizard for Mac** | macOS | ≈ $99 | Metadata + carving | APFS, HFS+, NTFS, exFAT, FAT | Good test results, pause/resume, S.M.A.R.T. | No imaging, no bootable media |
| **Prosoft Data Rescue** | macOS/Windows | ≈ $99 | Metadata + carving | APFS, HFS+, NTFS, FAT | Simple UI, imaging, bootable media | Lowest results in comparative testing |
| **MiniTool Mac Data Recovery** | macOS | ≈ $79 | Carving-heavy | APFS, HFS+, NTFS, FAT, exFAT | Very high raw file-detection counts | Slowest scans, no S.M.A.R.T. |
| **DiskWarrior** | macOS | ≈ $119 | Directory rebuild (repair, not recovery) | HFS+ (APFS support limited) | Rebuilds HFS+ catalog B-trees | Repair-only niche, APFS lag |
| **Scalpel / Foremost / bulk_extractor** | Linux-centric CLIs | Open source | Config-file-driven header/footer carving | FS-agnostic | Scalpel: fast multi-pass carving from a text config; bulk_extractor: feature extraction (emails, URLs) + carving | Scalpel is unmaintained; naive carving, no validation, no fragment handling |
| **The Sleuth Kit / Autopsy** | Cross-platform | Open source (IPL/CPL/Apache) | Forensic FS parsing | NTFS, FAT, exFAT, ext2-4, HFS+, APFS (via libtsk apfs), UFS, YAFFS2, ISO9660 | Reference implementation for "walk the FS and list unallocated entries"; timeline; hashing | Forensics workflow, not consumer recovery; no carving beyond `tsk_recover` |

## 2. Feature inventory — union of everything on the market

This is the checklist Reclaim should be measured against. Bold = table stakes for a "feature-packed" tool.

**Scanning**
- **Quick scan** (parse filesystem, list deleted entries with names/paths/timestamps)
- **Deep scan** (signature carving across every sector)
- **Lost-partition / lost-volume search** (boot-sector and superblock hunting)
- **Combined results view** (metadata-recovered files and carved files merged and de-duplicated)
- Scan of a specific volume, a whole disk, a raw file, or an image (`.dmg`, `.img`, `.iso`, E01, VMDK, sparsebundle)
- **Pause / resume / save session** to disk; reopen days later
- **Live results streaming** (files appear and are previewable while scan continues)
- Filters: type, size, date, path, recoverability score, "only deleted" vs "all"
- Recovery-chance estimate per file (contiguous & unallocated = high; partially overwritten = low)

**Imaging**
- **Byte-to-byte image** of a device to a file, with **bad-sector map** and skip/retry policy (multi-pass: fast pass first, retry bad regions later with smaller block sizes)
- Sparse images; **resume interrupted imaging**
- Entropy map (shows encrypted/random vs empty vs data regions)
- Forensic containers (E01, AFF4) with hashes — nice to have
- Hardware-imager integration (DeepSpar, PC-3000) — out of scope

**Filesystem breadth** (see doc 04)
- Apple: **APFS** (incl. snapshots, encrypted volumes when unlocked, Fusion), **HFS+** (journaled, case-sensitive), HFS
- Microsoft: **NTFS**, **exFAT**, **FAT12/16/32**, ReFS
- Linux: **ext2/3/4**, XFS, Btrfs, F2FS, JFS, ReiserFS
- Others: UFS/UFS2, ZFS, ISO9660/UDF, VMFS
- Partitioning: **GPT**, **MBR**, **APM**, hybrid MBR/GPT
- Containers/volume managers: APFS containers, Core Storage, LVM2, mdadm, Windows LDM & Storage Spaces, Synology SHR
- Encryption (unlocked or with key): FileVault 2 (APFS-encrypted), BitLocker, LUKS, VeraCrypt

**RAID**
- RAID 0/1/5/6/10 with auto-detect of stripe size, order, parity rotation
- Manual RAID builder; virtual RAID that the scanners run on top of

**File-type breadth** (see doc 05)
- 400+ signatures across images (incl. every camera RAW), video, audio, documents, archives, databases, mail, code, executables, disk images
- **Format-aware validation** (decode headers, verify structure) to reject false positives
- **Fragment reassembly** for JPEG and MP4/MOV (the "fragmented video reconstruction" Disk Drill markets)
- Original-filename recovery from embedded metadata (EXIF, ID3, Office core.xml)

**Output & UX**
- **Preview** (images, video first frame, text/PDF, audio) before recovery
- Recover to a chosen destination, preserving path structure; refuse to recover to source
- Hash & verify recovered files
- Reports (HTML/JSON/CSV) listing found and recovered files
- **S.M.A.R.T. monitoring** and health warnings that nudge towards imaging
- Bootable / Recovery-Mode operation for the boot disk
- "Recovery Vault" style deletion journaling (opt-in background agent that records metadata of deleted files so later recovery is 100 %) — Disk Drill's differentiator, a separate small product

**Also-ran / mobile**
- iOS device recovery (via backups), Android — out of scope for v1

## 3. Where the incumbents are weak (Reclaim's opportunity)

1. **PhotoRec has no names, no preview, no APFS metadata recovery.** A carving engine as good as PhotoRec's *plus* APFS/HFS+ metadata recovery *plus* a real preview is a genuine step up for the open-source world.
2. **Commercial Mac tools are opaque about what they actually do.** No published signature list, no published recovery-rate methodology. Reclaim can publish its signature catalog and test corpus (doc 09).
3. **Fragmented-file recovery is treated as magic.** Only Disk Drill markets it; R-Studio/UFS do it partially. A principled implementation for JPEG/HEIC/MP4/MOV (doc 05 §5) is a concrete differentiator, and photos/video are what Mac users actually lose.
4. **APFS snapshots and checkpoint history are under-used.** APFS is copy-on-write; old checkpoints and object-map versions can point at deleted files with full names. Most tools do a shallow job here.
5. **Nobody does scripting well.** A first-class CLI with JSON output, exit codes and session files makes Reclaim automatable (batch recovery for a photo studio, CI-tested behaviour).
6. **Free tiers are punitive.** Whatever the business model, a free tier that actually recovers something is a marketing weapon.

## 4. Positioning options

| Option | Description | Trade-off |
|--------|-------------|-----------|
| A. Open-source (permissive, MIT/Apache-2) CLI + GUI | Community credibility, contributions of signatures, easy to package via Homebrew | No direct revenue; must avoid GPL code (PhotoRec) — see doc 11 |
| B. Open core | Rust core + CLI open; GUI, RAID and video-reassembly paid | Common in dev tools; clear line needed |
| C. Closed freemium like Disk Drill | Highest revenue ceiling | You compete on marketing against companies with 10+ years of SEO |

The documents assume **A or B** (open Rust core + CLI). Nothing in them prevents C.

## 5. Sources

- [Disk Drill 6 release notes (CleverFiles)](https://www.cleverfiles.com/help/disk-drill-6.html)
- [Disk Drill: Target Disk Mode / Apple Silicon recovery guide](https://www.cleverfiles.com/help/data-recovery-target-disk-mode.html)
- [PhotoRec overview (CGSecurity)](https://www.cgsecurity.org/wiki/photoRec)
- [PhotoRec data carving internals (CGSecurity)](https://www.cgsecurity.org/wiki/PhotoRec_Data_Carving)
- [TestDisk 7.2 documentation](https://www.cgsecurity.org/testdisk_doc/)
- [UFS Explorer Professional Recovery feature list](https://www.ufsexplorer.com/ufs-explorer-professional-recovery/)
- [UFS Explorer RAID recovery](https://www.ufsexplorer.com/ufs-explorer-raid-recovery/)
- [SoftwareHow: 9 best Mac data recovery apps, tested](https://www.softwarehow.com/best-mac-data-recovery/)
- [Recuva review (7datarecovery)](https://ratings.7datarecovery.com/recuva-review/)
- [Scalpel (sleuthkit/scalpel, unmaintained)](https://github.com/sleuthkit/scalpel)
- [Stellar: how the T2 chip impacts Mac data recovery](https://www.stellarinfo.com/article/t2-chip-data-recovery.php)
- [Hetman: APFS data recovery algorithm and structure](https://hetmanrecovery.com/recovery_news/data-recovery-algorithmfile-system-apfs-and-its-structure.htm)
- [Wondershare: Mac system drive not visible to recovery software](https://recoverit.wondershare.com/faq/mac-system-drive-not-visible-recovery-software.html)
- [F-Response: Apple OSX and Full Disk Access](https://f-response.com/blog/apple_full_disk_access)
