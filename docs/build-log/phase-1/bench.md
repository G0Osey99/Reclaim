# Phase 1 benchmark — reclaim vs PhotoRec

_Generated 2026-09-12 15:03:32-0400 · exact SHA-256 content recall against each image's ground-truth sidecar (docs/plan/09 §3)._

| image | reclaim recall | photorec recall | reclaim prec | reclaim time | reclaim peak RSS | photorec time |
|---|---|---|---|---|---|---|
| apfs-delete-history | 82.5% | 70.0% | 89.2% | 2.40s | 129360 KB | 0.07s |
| exfat-camera-delete | 100.0% | 58.3% | 100.0% | 1.09s | 127904 KB | 0.05s |
| fat32-usb-delete | 85.0% | 50.0% | 85.0% | 1.10s | 130064 KB | 0.11s |
| hfsplus-delete | 100.0% | 85.0% | 100.0% | 1.32s | 132800 KB | 0.14s |

## Per-family content recall (reclaim)

- **apfs-delete-history**: jpeg 9/10, mp4 9/10, png 9/10, txt 6/10
- **exfat-camera-delete**: jpeg 12/12, mp4 12/12, png 12/12, txt 12/12
- **fat32-usb-delete**: jpeg 10/10, mp4 10/10, png 10/10, txt 4/10
- **hfsplus-delete**: jpeg 10/10, mp4 10/10, png 10/10, txt 10/10

## Notes

- Target (build-guide Phase-1 gate): reclaim `content_recall` ≥ PhotoRec on every image — **MET**.
- Partial credit (longest-correct-prefix) needs the original file bytes; the sidecar stores only hashes, so it is reported N/A this phase.
- Text (`.txt`) exact recall depends on cluster slack being zeroed; where PhotoRec and reclaim both carve headerless text statistically, neither reproduces an exact hash when the on-disk extent differs from the recorded size.
