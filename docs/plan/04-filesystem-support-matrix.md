# 04 — Filesystem, Partition & Container Support Matrix

Tiers: **T1** = must ship in v1.0 with metadata recovery; **T2** = v1.x metadata recovery; **T3** = detect + carve only (block-size aware), metadata later or never. "Carve only" is always available for every filesystem because carving ignores the FS.

## 1. Partition schemes & containers

| Scheme | Tier | Detection anchor | Recovery notes |
|--------|------|------------------|----------------|
| GPT | T1 | `EFI PART` at LBA 1; backup header at last LBA; CRC32 of header & entries | Use backup when primary corrupt; scan for orphan entries; recognize Apple type GUIDs (APFS `7C3457EF-…`, HFS+ `48465300-…`, Core Storage `53746F72-…`, Boot `426F6F74-…`) |
| MBR / hybrid MBR | T1 | `0x55AA` at 510; 4 entries at 446; EBR chains | Many SD cards / cameras; protective MBR on GPT disks |
| APM (Apple Partition Map) | T2 | `ER` at 0, `PM` entries | Legacy PowerPC disks, some older iPods/externals |
| APFS container | T1 | `NXSB` magic at block 0; checkpoint descriptor area | Container holds up to 100 volumes; must parse checkpoints & object map to find volumes (see §3.1) |
| Core Storage | T2 | Core Storage GPT type; CS metadata at start of PV | Pre-APFS FileVault & Fusion; logical volume is an HFS+ volume at an offset in the PV |
| LVM2 | T2 | `LABELONE` in first 4 sectors of PV; metadata area text | Linear/striped LVs → OffsetView / RaidView |
| Linux mdadm | T2 | superblock v0.90 (end of device), v1.x (offsets 0/4K/end) | Feeds RAID engine with level, chunk, role order |
| Windows LDM (dynamic disks) | T3 | `PRIVHEAD` | Rare on Macs |
| Windows Storage Spaces | T3 | — | Rare; UFS Explorer-only territory |
| Synology SHR / QNAP | T3 | mdadm + LVM layered | Falls out of mdadm + LVM2 support |
| Apple Software RAID (AppleRAID) | T2 | AppleRAID GPT type, header at end of member | Mirror/stripe/concat |
| Fusion Drive | T2 | APFS container spanning 2 devices (Fusion) or Core Storage | APFS Fusion: tier-2 device holds extra blocks referenced by container |

## 2. Filesystems

| Filesystem | Tier | Probe | Block size source | Deleted-entry recovery method | Hard parts |
|------------|------|-------|-------------------|-------------------------------|------------|
| **APFS** | T1 | `NXSB` / `APSB` magic, Fletcher-64 checksum | `nx_block_size` (4096 typical) | (a) Walk current FS B-tree for live entries; (b) **walk older checkpoints**: each checkpoint's superblock → object map → volume superblock → root tree snapshot of that transaction; entries present in an old txn but absent now = deleted with full name/path/extents; (c) **snapshots** (`apfs_snap_meta` tree) give consistent older views; (d) orphan inode records in unallocated blocks (node magic check) | Encryption (volume keys — only when OS has unlocked; see doc 06); extent references via `j_file_extent`; sealed system volume (read-only, irrelevant to data); clones & compression (decmpfs in xattr); 64-bit object ids; checksum every block |
| **HFS+ / HFSX** | T1 | `H+`/`HX` at 1024; alt VH at size−1024 | Allocation block size in VH | Catalog B-tree: deleted records are removed, but **B-tree node slack** often retains record bytes → scan catalog file nodes for stale records; journal (`.journal`) replay history; unallocated blocks carve. Also parse `Extents Overflow` for fragmented files | Resource forks, compression (decmpfs), hard links via `\0\0\0\0HFS+ Private Data`, unicode decomposition of names |
| HFS (classic) | T3 | `BD` at 1024 | VH | Read-only listing | 16-bit limits; ignore except for vintage media |
| **NTFS** | T1 | `NTFS    ` OEM at +3 | Bytes/sector × sectors/cluster | `$MFT` records with `FILE` magic and in-use flag clear = deleted; data runs decode; `$MFTMirr`; orphan MFT records found by scanning for `FILE0`; `$LogFile` and `$UsnJrnl:$J` give names/history; `$I30` index slack retains names | Compressed/sparse runs, ADS, resident data, reparse points, name in `$FILE_NAME` vs `$I30` |
| **exFAT** | T1 | `EXFAT   ` at +3 | Bytes/sector, sectors/cluster shifts | Directory entries with **in-use bit (0x80) cleared** (types 0x05/0x40/0x41) still hold name, size, first cluster; `NoFatChain` flag → contiguous; allocation bitmap (`$BITMAP`) distinguishes free | Fragmented deleted files lose chain info when FAT entries are reused; upcase table; checksum of entry set |
| **FAT12/16/32** | T1 | BPB + `0x55AA`; FS type string | BPB | Entries starting `0xE5` = deleted (first char lost); LFN entries usually survive → full name; first cluster + size known; chain only recoverable if FAT entries not zeroed (they are, on delete — assume contiguous); backup boot sector at 6 | Cluster chain loss; 2-byte high cluster word on FAT32 deletes (Windows zeroes it); dual FATs may differ |
| ReFS | T3 | `ReFS` at +3 | 64 KiB / 4 KiB | Minimal; carve | Proprietary, versioned |
| **ext2/3/4** | T2 | `0xEF53` at +0x438 | `s_log_block_size` | ext2: inode retains block pointers after delete → full recovery. ext3/4: inode's block map/extent tree is zeroed on delete → **use journal** (`jbd2`) for old inode copies, directory entry slack (deleted dirents are "absorbed" into the previous entry's rec_len but bytes remain) → names; extents in unallocated blocks via extent-header magic `0xF30A` | Flex groups, inline data, encryption (fscrypt), htree dirs |
| XFS | T2 | `XFSB` | `sb_blocksize` | Inode cores remain (unlinked); B+tree extents; dir blocks keep stale entries; `xfs_logprint`-style log parsing | Multiple AGs, v5 CRCs |
| Btrfs | T2 | `_BHRfS_M` at 64 KiB (+ copies at 64 MiB, 256 GiB) | `sectorsize`/`nodesize` | COW → old tree roots reachable via `backup_roots` in superblock and by scanning for tree nodes with matching fsid; subvolume snapshots | Multi-device, RAID profiles, compression (zlib/lzo/zstd), checksums |
| F2FS | T3 | `0xF2F52010` | 4 KiB | Detect + carve | Flash-oriented, complex |
| JFS / ReiserFS | T3 | `JFS1` / `ReIsEr2Fs` | — | Detect + carve | Legacy; ReiserFS tail packing defeats carving of small files |
| UFS / UFS2 (FreeBSD, old macOS) | T3 | magic `0x00011954` / `0x19540119` at 8/64 KiB | superblock | Detect + carve; inode listing later | — |
| ZFS | T3 | uberblock `0x00bab10c` | `ashift` | Detect; label walk; carve | Very complex; pool import semantics |
| ISO 9660 / Joliet / UDF | T2 | `CD001` at 32 KiB; UDF anchor at sector 256 | 2048 | Read-only listing; recover from damaged images | Multi-session |
| VMFS 5/6 | T3 | `0xC001D00D` | 1 MiB | Detect + carve | Enterprise only |
| NWFS / NSS / HPFS | — | — | — | Not planned | — |

## 3. Deep-dive notes for the T1 filesystems

### 3.1 APFS recovery algorithm (the differentiator)

1. Read block 0 → `nx_superblock_t`; verify Fletcher-64 over the block; get `nx_block_size`, `nx_xp_desc_base/blocks`, `nx_omap_oid`, `nx_fs_oid[]`.
2. **Enumerate the checkpoint descriptor ring**: every `NXSB` found in the descriptor area is a historical container state with its own `nx_xid`. Sort by `xid` descending. The highest valid one is "current"; the others are history. **[Phase 3 correction]** The ring is *shallow* — its length is `nx_xp_desc_blocks` and each checkpoint costs one checkpoint-map + one NXSB, so a 512 MiB test container's ring holds only **~4 checkpoints**, not "a few dozen to a few hundred". Churn does not deepen it; it evicts old checkpoints. Checkpoint history therefore recovers only very recent deletions; the workhorse for deleted-file recovery is the orphan scan (step 7).
3. For each checkpoint (newest first): resolve container object map (`omap` B-tree, physical) → for each volume oid, resolve `apfs_superblock_t` → its own omap → root FS tree (`j_*` records keyed by `(obj_id, type)`).
4. Walk FS trees; collect inodes (`APFS_TYPE_INODE`), dir records (`APFS_TYPE_DIR_REC`, which carry names and parent ids), file extents (`APFS_TYPE_FILE_EXTENT`), xattrs (compressed data, resource forks).
5. Diff: a `(inode, name, parent)` present at xid *k* but absent at the newest xid ⇒ **deleted between k and now**. Report with the path from the *old* tree and the extents from the old extent records. **[Phase 3 correction]** Do *not* use the space-manager bitmap as the "overwritten?" signal: APFS re-allocates a freed block (usually to metadata) almost immediately while the old bytes survive, so a spaceman-*allocated* block is frequently still recoverable. Score against **blocks referenced by current live files** instead (a live file now occupying the block ⇒ truly overwritten ⇒ lower score). The space-manager bitmap is still parsed for free-space reporting.
6. **Snapshots**: `apfs_snap_metadata` tree lists snapshot xids and their `sblock_oid`; each is a full consistent older view — walk the same way (Time Machine local snapshots on the boot volume are a goldmine).
7. **Orphan scan**: scan blocks for B-tree node headers (`obj_phys_t` with valid checksum and subtype FSTREE) to recover records from trees that have dropped out of every checkpoint. **[Phase 3 correction]** Scan **every block not referenced by a live file**, not only space-manager-*free* blocks — freed FS-tree nodes are quickly re-allocated to new metadata while the old bytes remain, so a free-only scan misses most deleted records. The valid Fletcher-64 + FSTREE-subtype check is a strong enough filter to scan broadly. This is the primary deleted-file recovery path in practice (it took the `apfs-many-deletes` bench from 8% → 100%).
8. Encrypted volumes: records are encrypted per-block; only proceed when running on a Mac that has the volume unlocked (read through the unlocked `/dev/rdiskNsM`? — see doc 06 §4, verify empirically) or with the volume key via the crypto layer.

Reference: Apple, *Apple File System Reference* (developer.apple.com, PDF). Cross-check with the `apfs-fuse` and `libfsapfs` open-source implementations (read for understanding; do not copy GPL/LGPL code into an Apache-licensed crate — doc 11).

### 3.2 HFS+ specifics
- Catalog file is a B-tree; when a record is deleted the node is rewritten compacted, but **the freed tail bytes of the node are not zeroed** — scan every node's slack for `kHFSPlusFileRecord`/`kHFSPlusFolderRecord` structures with plausible CNIDs and names.
- The **journal** (`jhdr` at the journal info block) contains recent metadata block writes including previous versions of catalog nodes — replay backwards for deleted records.
- Fork data: 8 extents inline, the rest in the Extents Overflow B-tree keyed by CNID + fork type + start block. Deleted files' overflow records are also removed, so very fragmented large files recover partially unless found in journal.

### 3.3 NTFS specifics
- Parse `$MFT` itself first (its data runs are in record 0). Walk every 1024-byte record (`FILE` magic, sequence no., flags). Flag bit 0 clear = not in use = deleted candidate; attributes `$FILE_NAME` (names), `$DATA` (runs or resident), `$STANDARD_INFORMATION` (timestamps).
- For names that are gone from the MFT, mine `$I30` index allocation slack and `$UsnJrnl:$J` (change journal records have file names + reference numbers).
- Scan the whole volume for `FILE0` records to find orphaned MFT fragments after a re-format.

### 3.4 exFAT & FAT specifics
- exFAT: directory entry set = File(0x85) + Stream(0xC0) + Name(0xC1…); deleted → 0x05/0x40/0x41. `NoFatChain`=1 ⇒ contiguous from first cluster for `DataLength` — most camera files. If 0, follow FAT; if the FAT chain is zeroed, assume contiguous and flag `Suspect`.
- FAT32: name's first byte → 0xE5; LFN entries (attr 0x0F) precede and survive with full long name. High 16 bits of first cluster zeroed by Windows; macOS does not zero. Heuristic: search for a cluster with a matching header near the low-16-bit candidates.

## 4. Encryption & unlock layers

| Layer | Tier | Approach |
|-------|------|----------|
| APFS encrypted volume (FileVault 2 on APFS) | T1 (via OS) / T3 (own crypto) | Rely on macOS having unlocked the volume; otherwise prompt for password/recovery key and implement key unwrapping (KEK/VEK, AES-XTS) — substantial work, later |
| Core Storage FileVault (HFS+) | T3 | Same idea, older format |
| T2 / Apple Silicon internal SSD hardware encryption | n/a | Transparent when booted on the same Mac; impossible otherwise (doc 06) |
| BitLocker | T2 | Password / recovery key / BEK; AES-CBC/XTS; dislocker as reference |
| LUKS1/2 | T2 | PBKDF2/Argon2 → master key; AES-XTS |
| VeraCrypt | T3 | Header brute-force of cipher cascades |
| Encrypted DMG / sparsebundle | T2 | CDSA/`encrcdsa` header; AES-128/256 |

## 5. Image container formats the block layer must open

Raw (`.img/.dd/.bin/.iso`), `.dmg` UDIF (uncompressed `UDRO`, zlib `UDZO`, bzip2 `UDBZ`, lzfse `ULFO`, lzma `ULMO` — parse `koly` trailer + `mish` blocks), sparsebundle/sparseimage, VMDK (flat + sparse extents), VDI, VHD/VHDX, QCOW2, EnCase E01/Ex01 (libewf format; zlib chunks + CRCs), AFF4 (zip-based, later), split images (`.001`, `.002`).
