# Phase 2 benchmark — named + content recall (reclaim vs PhotoRec)

_Generated 2026-09-12 16:55:44-0400 · content_recall = exact SHA-256 of recovered bytes; named_recall = deleted file recovered at the right path with matching content (docs/plan/09 §3). PhotoRec is carve-only (no names)._

| image | fs | named_recall | content_recall | photorec content | reclaim prec | time | peak RSS |
|---|---|---|---|---|---|---|---|
| apfs-delete-history | APFS | n/a (Phase 3) | 82.5% | 70.0% | 89.2% | 2.10s | 115488 KB |
| exfat-camera-delete | ExFAT | 100.0% | 100.0% | 58.3% | 47.5% | 0.96s | 104688 KB |
| fat32-usb-delete | MS-DOS FAT32 | 100.0% | 100.0% | 50.0% | 44.9% | 0.90s | 104800 KB |
| hfsplus-delete | Journaled HFS+ | n/a (Phase 3) | 100.0% | 85.0% | 100.0% | 1.15s | 104944 KB |
| ntfs-delete | NTFS | 100.0% | 100.0% | 66.7% | 100.0% | 0.10s | 47936 KB |

## Per-family content recall (reclaim)

- **apfs-delete-history**: jpeg 9/10, mp4 9/10, png 9/10, txt 6/10
- **exfat-camera-delete**: jpeg 12/12, mp4 12/12, png 12/12, txt 12/12
- **fat32-usb-delete**: jpeg 10/10, mp4 10/10, png 10/10, txt 10/10
- **hfsplus-delete**: jpeg 10/10, mp4 10/10, png 10/10, txt 10/10
- **ntfs-delete**: jpeg 8/8, png 8/8, txt 8/8

## Gates

- content_recall ≥ PhotoRec on every image: **MET**.
- named_recall ≥ 0.95 on every Phase-2-supported (exFAT/FAT/NTFS) image: **MET**.
- APFS/HFS+ images show named_recall `n/a` (their metadata engines land in Phase 3); their content_recall is the carver's, unchanged from Phase 1.
