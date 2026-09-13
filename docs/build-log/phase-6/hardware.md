# Phase 6 — Hardware QA checklist (doc 09 §6)

These items need **Ryker present** for the `sudo` / Full Disk Access / FileVault
prompts, physical media, hot-unplug, a bad-sector drive, and a second Mac (or
fresh user/VM). This file is the **runbook + evidence log**: run each item with
Ryker, paste the command output under it, and set the verdict.

**Nothing here is pre-filled PASS.** Verdicts start `PENDING` and are set to
`PASS`/`FAIL`/`N/A` only after the step actually runs. `N/A` requires a stated
reason (hardware not available).

Conventions: the CLI under test is the universal Recovery binary
(`scripts/build-recovery.sh` → `dist/reclaim-<ver>-universal-apple-darwin/reclaim`)
or `target/release/reclaim`. Read-only always: point it only at RECLAIM-TEST media
(Part 5.2). Capture evidence with `reclaim … --json` where useful.

Host of record: Apple Silicon, macOS 26.6.2, FileVault __ (fill), date __.

---

## 1. Apple Silicon Mac, FileVault on

**1a. Normal boot — list / info / quick scan of the Data volume (read-only).**

```bash
sudo -v                                   # Ryker enters password
reclaim list
reclaim info disk3s5                       # the Data volume (from `reclaim list` roles)
sudo reclaim scan disk3s5 --quick --session /Volumes/RECLAIM-EXT/sess-data
reclaim results /Volumes/RECLAIM-EXT/sess-data --deleted-only --limit 20
# Confirm the source is untouched:
reclaim --prove-readonly info disk3s5 2>&1 | tail -3   # all opens O_RDONLY
```

Expected: `list` shows the internal container + Data role; `info` prints size, FS,
and a health verdict; the quick scan lists named live+deleted entries; the
prove-readonly log shows only `O_RDONLY`. (Deep-carving the internal SSD is
expected to recover little — TRIM; that is the honest chances story, not a fail.)

- Verdict: **PENDING** · evidence:

**1b. CLI from a Recovery Terminal (Phase-4 USB procedure).**

Prep (you, on the working Mac):
```bash
scripts/build-recovery.sh                                   # universal, system-libs-only
scripts/recovery-usb.sh dist/reclaim-*-universal-apple-darwin/reclaim /Volumes/RECLAIM-EXT
```
Ryker boots Recovery (hold power → Options → Continue), **Utilities → Terminal**,
then follows [../../recovery-mode.md](../../recovery-mode.md):
```sh
/Volumes/RECLAIM-EXT/reclaim-recovery/reclaim list
/Volumes/RECLAIM-EXT/reclaim-recovery/reclaim info disk0
/Volumes/RECLAIM-EXT/reclaim-recovery/reclaim scan disk3s5 --session /Volumes/RECLAIM-EXT/rec-sess
```
Expected: runs as root with no FDA dance; `list`/`info`/`scan` all work; a
FileVault Data volume must be unlocked in Disk Utility first.

- Verdict: **PENDING** · this also closes "docs/recovery-mode.md executed on hardware" (Part 6).

## 2. Sacrificial SD cards from ≥ 2 cameras vs Disk Drill trial + PhotoRec

For each of two cameras (e.g. Canon + Sony/GoPro/DJI): format the card **in the
camera**, shoot throwaway photos/video, delete some in-camera, then:

```bash
diskutil list                              # find the card's diskN (RECLAIM-TEST only!)
sudo reclaim scan diskN --session ~/sd-$CAM
reclaim results ~/sd-$CAM --deleted-only
reclaim recover ~/sd-$CAM /Volumes/RECLAIM-EXT/rec-$CAM --deleted-only --preserve-paths --verify
# Competitors on the SAME card:
photorec /log /d ~/pr-$CAM /cmd /dev/diskN search
# Disk Drill (trial): scan the card, note the recoverable count + whether names appear.
```
Record, per camera: reclaim named-recovery count, reclaim carved count, PhotoRec
count (no names), Disk Drill count (+ whether it shows names). Reclaim should
recover **with names/paths** and be ≥ PhotoRec on carved count.

- Camera A (____): Verdict **PENDING** · counts: reclaim __ / PhotoRec __ / Disk Drill __
- Camera B (____): Verdict **PENDING** · counts: reclaim __ / PhotoRec __ / Disk Drill __

## 3. USB drive with bad sectors → image with map → scan the image

Use a real failing/bad-sector HDD or stick if one is on hand (Part 5.2 "optional
but valuable").
```bash
sudo reclaim info diskN                    # expect a marginal/FAILING health verdict
sudo reclaim image diskN /Volumes/RECLAIM-EXT/bad.img --retries 5 --reverse-pass
reclaim verify-image /Volumes/RECLAIM-EXT/bad.img            # inspect the .reclaim-map
reclaim scan /Volumes/RECLAIM-EXT/bad.img --session ~/bad-sess
```
Expected: the imager finishes despite errors, the sidecar map marks the bad
ranges, and the scan runs on the image. If no failing drive is available, mark
**N/A (no bad-sector drive)** — the imager's bad-sector handling is already proven
on synthetic media by the `fault::injects_bad_sectors_and_zeroes` unit test
(FaultInjector), which is CI-gated.

- Verdict: **PENDING / N/A** · evidence:

## 4. NTFS & ext4 externals, Windows-formatted exFAT, FAT32 stick

Docker-made sticks are fine for NTFS/ext4 (or a Windows-formatted drive).
```bash
for d in <ntfs diskN> <ext4 diskN> <exfat diskN> <fat32 diskN>; do
  sudo reclaim scan "$d" --session ~/ext-$d
  reclaim results ~/ext-$d --deleted-only --limit 10
done
```
Expected: each mounts, is identified by FS, and lists deleted entries by name.

- NTFS: **PENDING** · ext4: **PENDING** · exFAT (Windows): **PENDING** · FAT32: **PENDING**

## 5. Hot-unplug → resume · recover 20+ GB with verify · same-disk refusal

**5a. Hot-unplug mid-scan → resume.**
```bash
sudo reclaim scan diskN --session ~/unplug   # yank the drive mid-scan
# expect: exit code 4 + "device disappeared … --resume" message
reclaim ; echo "exit=$?"                       # confirm 4
# replug, then:
sudo reclaim scan diskN --session ~/unplug --resume
```
- Verdict: **PENDING** (exit 4 seen: __, resume completed: __)

**5b. Recover 20+ GB with --verify.**
```bash
sudo reclaim scan <big source> --session ~/big
reclaim recover ~/big /Volumes/RECLAIM-EXT/big-out --all --verify
# expect: per-file hash verify, a manifest.json, ≥ 20 GB copied, 0 verify failures
```
- Verdict: **PENDING** (GB recovered: __, verify failures: __)

**5c. Same-disk refusal (CLI + app).**
```bash
# CLI: destination on the source disk must be refused (exit 5).
reclaim recover ~/big /Volumes/<SAME-AS-SOURCE>/out --all ; echo "exit=$?"   # expect 5
```
App: pick a recover destination on the source disk → the recover sheet must show
the live same-disk refusal and disable Recover.
- CLI refusal (exit 5): **PENDING** · App refusal: **PENDING**

## 6. Fresh macOS VM (or second user): DMG → Gatekeeper → helper → FDA → scan

On a second Mac / fresh user / VM that has never seen this code:
1. Download `Reclaim-1.0.0.dmg` from the GitHub release.
2. Open it → drag to /Applications → launch. **Gatekeeper must pass** (notarized,
   stapled): `spctl --assess -vv /Applications/Reclaim.app` → "accepted, source=Notarized Developer ID".
3. Onboarding: **Install helper** → approve in System Settings → Login Items;
   **grant Full Disk Access**; the pills flip to done.
4. Scan an external SD/USB card; confirm results stream with thumbnails and a
   recover to a different disk works.

> Blocked until the notarized DMG exists (Ryker — paid Developer ID, Part 5.3).
> Until then, the local `appledev`-signed build installs the helper on *this* Mac
> only (Phase 5).

- Gatekeeper: **PENDING** · helper install: **PENDING** · FDA: **PENDING** · scan: **PENDING**

---

## Summary

| # | Item | Verdict |
|---|---|---|
| 1a | Apple Silicon, FileVault — list/info/quick (normal boot) | PENDING |
| 1b | CLI from Recovery Terminal | PENDING |
| 2 | ≥2 camera SD cards vs Disk Drill + PhotoRec | PENDING |
| 3 | Bad-sector drive → image+map → scan | PENDING / N/A |
| 4 | NTFS / ext4 / Windows exFAT / FAT32 externals | PENDING |
| 5a | Hot-unplug → resume | PENDING |
| 5b | Recover 20+ GB with --verify | PENDING |
| 5c | Same-disk refusal (CLI + app) | PENDING |
| 6 | Fresh VM: DMG → Gatekeeper → helper → FDA → scan | PENDING (needs notarized DMG) |

No unexplained FAIL: to satisfy the phase gate, every row must read PASS, or N/A
with the stated hardware reason.
