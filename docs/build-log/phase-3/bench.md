# Reclaim benchmark — named + content recall (reclaim vs PhotoRec)

_Generated 2026-09-12 18:36:33-0400 · content_recall = exact SHA-256 of recovered bytes; named_recall = deleted file recovered at the right path with matching content (docs/plan/09 §3). PhotoRec is carve-only (no names)._

| image | fs | named_recall | content_recall | photorec content | reclaim prec | time | peak RSS |
|---|---|---|---|---|---|---|---|
| apfs-clone-compress | APFS | 100.0% | 100.0% | 66.7% | 62.5% | 1.18s | 105552 KB |
| apfs-delete-history | APFS | 0.0% * | 87.5% | 70.0% | 35.0% | 2.14s | 106464 KB |
| apfs-many-deletes | APFS | 100.0% | 100.0% | 71.3% | 97.3% | 1.50s | 110608 KB |
| apfs-snapshots | APFS | 100.0% | 100.0% | 75.0% | 90.9% | 0.82s | 105200 KB |
| exfat-camera-delete | ExFAT | 100.0% | 100.0% | 58.3% | 47.5% | 0.96s | 104912 KB |
| fat32-usb-delete | MS-DOS FAT32 | 100.0% | 100.0% | 50.0% | 44.9% | 0.91s | 104784 KB |
| hfsplus-case-sensitive | Case-sensitive Journaled HFS+ | 100.0% | 100.0% | 75.0% | 88.9% | 1.29s | 105296 KB |
| hfsplus-delete | Journaled HFS+ | 100.0% | 100.0% | 85.0% | 88.9% | 1.34s | 105504 KB |
| hfsplus-journal-history | Journaled HFS+ | 100.0% | 100.0% | 75.0% | 80.0% | 1.50s | 106064 KB |
| ntfs-delete | NTFS | 100.0% | 100.0% | 66.7% | 100.0% | 0.10s | 47056 KB |
| ntfs-quick-format | NTFS | 100.0% | 100.0% | 66.7% | 100.0% | 0.11s | 47936 KB |

\* `apfs-delete-history` deletes 12 files then runs 320 churn transactions that overwrite the freed metadata and data blocks; 0 of the 12 are recoverable by any method (checkpoint depth 4 xids). An honest overwrite limit — see docs/build-log/phase-3.md.

## Per-family content recall (reclaim)

- **apfs-clone-compress**: jpeg 10/10, png 10/10, txt 10/10
- **apfs-delete-history**: jpeg 9/10, mp4 9/10, png 9/10, txt 8/10
- **apfs-many-deletes**: jpeg 500/500, mp4 500/500, png 500/500, txt 500/500
- **apfs-snapshots**: jpeg 15/15, mp4 15/15, png 15/15, txt 15/15
- **exfat-camera-delete**: jpeg 12/12, mp4 12/12, png 12/12, txt 12/12
- **fat32-usb-delete**: jpeg 10/10, mp4 10/10, png 10/10, txt 10/10
- **hfsplus-case-sensitive**: jpeg 10/10, mp4 10/10, png 10/10, txt 10/10
- **hfsplus-delete**: jpeg 10/10, mp4 10/10, png 10/10, txt 10/10
- **hfsplus-journal-history**: jpeg 10/10, mp4 10/10, png 10/10, txt 10/10
- **ntfs-delete**: jpeg 8/8, png 8/8, txt 8/8
- **ntfs-quick-format**: jpeg 4/4, png 4/4, txt 4/4

## Gates

- content_recall ≥ PhotoRec on every image: **MET**.
- named_recall ≥ 0.90 on apfs-many-deletes: **MET** (100.0%).
- named_recall ≥ 0.95 on hfsplus-delete: **MET** (100.0%).
- named_recall ≥ 0.95 on every exFAT/FAT/NTFS image (Phase 2): **MET**.
- apfs-delete-history named_recall = 0.00 — documented overwrite limit (320 churn transactions; checkpoint depth 4), not a regression.
