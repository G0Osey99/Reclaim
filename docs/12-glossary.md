# 12 — Glossary

| Term | Meaning |
|------|---------|
| **Allocation bitmap** | Per-block/cluster map of used vs free space kept by a filesystem (APFS space manager, exFAT `$BITMAP`, NTFS `$Bitmap`, ext block bitmaps, FAT table). Lets the carver focus on free space and score overwrite risk. |
| **APFS checkpoint** | A consistent container state recorded in the checkpoint descriptor/data areas with a transaction id (`xid`). Older checkpoints remain on disk until the ring wraps — free history. |
| **Block size / cluster** | Smallest allocation unit of a filesystem (APFS 4 KiB; exFAT camera cards often 128 KiB; NTFS 4 KiB). Files start on these boundaries, which is why carving on boundaries is fast and accurate. |
| **Byte-to-byte image** | Exact sector copy of a device to a file, including free space; scanning the image instead of the device protects failing hardware. |
| **Carving** | Finding files by content (headers/footers/structure) without using filesystem metadata. Yields data but not original names/paths. |
| **Checkpoint (session)** | Reclaim's saved scan progress allowing resume. |
| **Copy-on-write (COW)** | APFS/Btrfs/ZFS never overwrite blocks in place; old versions linger until space is reclaimed → recovery opportunity. |
| **Deep scan** | Full sequential read with carving + structure hunting. |
| **Extent** | A contiguous run of blocks belonging to a file (start, length). A file = list of extents. |
| **FDA** | Full Disk Access, the macOS TCC permission category. |
| **Fragment reassembly** | Reconstructing a file whose extents are non-contiguous when the filesystem no longer records them. |
| **Index of coincidence** | Statistic of byte repetition used to tell natural-language text from random/compressed data. |
| **ISOBMFF** | ISO Base Media File Format — the box/atom structure shared by MP4, MOV, HEIC, CR3, M4A. |
| **Lost volume / structure search** | Finding partitions/filesystems whose partition table entries are gone by scanning for boot sectors/superblocks. |
| **MFT** | NTFS Master File Table; each 1 KiB record describes one file. |
| **Metadata recovery / quick scan** | Parsing the filesystem's own structures to find deleted entries with names, paths, timestamps and extents. |
| **Object map (omap)** | APFS B-tree mapping virtual object ids (+xid) to physical block addresses. |
| **Orphan** | A metadata record (MFT record, APFS node, catalog record) found in free space that no live structure references. |
| **Recoverability score** | Reclaim's 0–100 estimate combining validator confidence, allocation state of extents, and engine provenance. |
| **Sealed System Volume** | macOS's read-only, cryptographically signed APFS system snapshot. Never contains user data. |
| **SEP** | Secure Enclave Processor; holds storage encryption keys on T2/Apple Silicon Macs. |
| **Signature** | Byte pattern identifying a file type at a known offset (magic number). |
| **SIP** | System Integrity Protection. |
| **Slack** | Unused bytes inside an allocated structure (B-tree node, directory block, last cluster of a file) that often still contain stale data. |
| **S.M.A.R.T.** | Drive self-reported health attributes. |
| **Snapshot** | Read-only point-in-time view of a volume (APFS, Btrfs, ZFS). Time Machine uses APFS local snapshots. |
| **Suspect / Truncated / Full** | Reclaim validity states: structure invalid or touches bad/overwritten blocks; ends before logical end; passes validation. |
| **TRIM / discard** | SSD command telling the controller blocks are free; afterwards they read as zeros, ending recovery chances. |
| **Validator** | Per-format code that checks a candidate file's structure and computes its true length. |
