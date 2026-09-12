# 06 — macOS Platform Notes

These are the facts that shape what a Mac recovery tool can and cannot do. Items that were once marked **[verify]** are widely reported claims; Phase 0 resolved them on the M0 hardware. The findings are recorded inline below and summarised in **§12 (Phase 0 verification results)**; the steps that still require `sudo`/sacrificial media are flagged there as Manual (needs Ryker), with a turnkey `scripts/platform-proofs.sh`.

## 1. Device enumeration

- **IOKit** (`IOServiceMatching("IOMedia")`) yields every block device with properties: `BSD Name` (`disk3s2`), `Size`, `Preferred Block Size`, `Whole`, `Leaf`, `Removable`, `Ejectable`, `Content` (partition type), parent chain giving `IOBlockStorageDevice` → model, vendor, serial, protocol (USB/SATA/NVMe/Thunderbolt/SD).
- **DiskArbitration** (`DADiskCopyDescription`) gives volume name, mount point, UUID, filesystem kind, and lets you register for appear/disappear callbacks (for hot-plugged SD cards).
- `diskutil list -plist` / `diskutil apfs list -plist` is an acceptable fallback (it's stable, but shelling out is slower). APFS container ↔ physical store ↔ volume relationships come from `diskutil apfs list` or IOKit `AppleAPFSContainer`/`AppleAPFSVolume` classes.
- APFS layout on a modern boot disk: `disk0` (physical) → `disk0s2` (APFS container physical store) → synthesized `disk3` (container) → `disk3s1` System (sealed, read-only snapshot mounted), `disk3s5` Data (user files), `disk3s2`… Preboot/Recovery/VM. **User data recovery targets the Data volume.**

## 2. Raw access, privileges, permissions

| Fact | Consequence |
|------|-------------|
| Block devices are `/dev/diskN` (buffered) and `/dev/rdiskN` (raw character device, unbuffered, requires reads aligned to the device block size). | Use `rdisk` for scanning: several times faster, and it bypasses the buffer cache so a failing drive isn't hammered by read-ahead. |
| Opening `/dev/rdiskN` for reading requires **root** (device nodes are `root:operator 0640`). | GUI needs a privileged helper (`SMAppService.daemon(plistName:)` registered helper with XPC, macOS 13+); CLI runs via `sudo`. |
| **Full Disk Access (TCC)** governs access to protected *user folders* (Desktop, Documents, Downloads, Mail, Messages, Time Machine backups, etc.) at the *file* level, and — since macOS 10.15 — is also required to read the raw device that backs the boot volume. **(Phase 0, §12 — two gates, verified):** the boot/container/Data `/dev/rdisk*` nodes have **two** independent gates: DAC (`root:operator 0640` → root *or* `operator` group) and TCC/FDA (uid-independent; root does **not** confer it). Critically, `sudo` satisfies DAC but *drops* the FDA attribution (the child re-parents away from the FDA-holding responsible process), so `sudo dd if=/dev/rdisk0` returns `EPERM` here even though FDA is granted. The working path is **non-sudo + `operator` group**. `reclaim doctor` should therefore check operator membership **and** FDA effectiveness, not just root. | Ship an onboarding step that deep-links to *System Settings → Privacy & Security → Full Disk Access* and checks status by attempting a read. |
| **SIP** does not block reading raw devices for root, but blocks writing to system locations and blocks debugger attach. Some third-party guides say "disable SIP for recovery" — treat that as *last resort* and never require it. | Design so that Reclaim never needs SIP off. |
| **System volume is sealed** (signed APFS snapshot). It contains no user data. | Never scan it by default; hide it behind an "advanced" toggle. |
| **Data volume of the running system is in use.** Scanning it is possible (read-only) but the OS keeps writing (logs, Spotlight, caches) and TRIM on the internal SSD makes deleted blocks return zeros quickly. | Prominent warning: "Stop using this Mac; recover to an external drive; consider booting from an external installer / Recovery Mode." |

## 3. Trim, SSDs, and what "deep scan" can find on Apple internal storage

- Internal Apple SSDs TRIM immediately; APFS issues discards on free. After a delete, the freed blocks typically read back as zeros within seconds to minutes. **Carving deleted content from an internal Apple SSD is mostly futile**; what still works is (a) APFS checkpoint/snapshot history where blocks are still referenced by an older transaction (not freed → not trimmed), (b) Time Machine local snapshots, (c) files in `.Trash`.
- External HDDs, SD cards, USB sticks and most camera media do **not** get TRIM'd (no UNMAP over USB mass storage in most bridges; cameras don't TRIM). That's where carving shines. Message this honestly in the UI (a "chances" indicator per source).

## 4. T2 and Apple Silicon: encryption at rest

- On T2 Intel Macs and all Apple Silicon Macs, the internal SSD is **always hardware-encrypted** by the Secure Enclave, FileVault or not. Keys never leave the SEP. Chip-off / transplant recovery is impossible.
- When you boot the same Mac (normal mode or Recovery Mode) the storage controller decrypts transparently; raw reads of the physical store return **plaintext container blocks** if FileVault is off, or **APFS-level-encrypted volume blocks** if FileVault is on until the volume is unlocked. **(Phase 0, §12 proof 3 — OPEN):** the device to read is the Data volume node — on this host `/dev/rdisk3s5` (`Encryption=true, FileVault=true`), its container `/dev/rdisk3`, physical store `/dev/rdisk0s2`. Read `NXSB` from the container/physical-store and `APSB` from the volume — never hardcode the synthesized numbers, resolve via `diskutil info /System/Volumes/Data`. Whether a raw read of the *unlocked* volume returns decrypted APFS (SEP decrypts transparently — expected) or ciphertext is **not yet empirically observed** (the read was DAC-blocked in the first run); close it via the operator-group + non-sudo path. The NXSB/APSB detector is validated on the unencrypted APFS golden image (5× NXSB, 66× APSB).
- Apple Silicon has **no Target Disk Mode**. *Share Disk* (from Recovery) exposes the volume over SMB — file-level only; useless for undelete/carving. Therefore recovering an Apple Silicon internal disk means **running Reclaim on that Mac**, either in normal boot (Data volume unlocked) or from Recovery Mode/an external boot disk.
- Intel Macs with T2 support Target Disk Mode; the volume still has to be unlocked with the user password or Personal Recovery Key on the host.
- Practical product consequence: ship a **CLI binary that runs from Recovery Mode Terminal** (static-ish, no GUI frameworks, no notarization gate in Recovery, signed anyway) plus instructions for building a bootable external macOS with Reclaim preinstalled. This is an explicit roadmap item (M5).

## 5. FileVault

- FileVault 2 on APFS = per-volume AES-XTS with a Volume Encryption Key wrapped by user Key Encryption Keys / recovery key. Unlock via `diskutil apfs unlockVolume` (or the mount prompt); after that, read through the OS-provided decrypted path (§4; see §12 proof 3).
- Implementing your own unlock (parsing the keybag, PBKDF2 over the password, AES-KW unwrap, XTS decrypt) is a T3 stretch goal that would let Reclaim read FileVault volumes from images taken on other machines — valuable for technicians, big effort.

## 6. APFS-specific recovery aids that only macOS gives you

- `tmutil listlocalsnapshots /` — local Time Machine snapshots; mount read-only at `/Volumes/com.apple.TimeMachine.*` via `mount_apfs -s <snap> /dev/disk3s5 /mnt`. A "snapshot browser" in Reclaim that diffs snapshots vs. current is a cheap, high-value feature that requires no raw access.
- `fs_usage`/`log show` are irrelevant; but `~/.Trash` and `/Volumes/*/.Trashes/<uid>` should be checked first (a "did you look in the Trash?" step avoids most support tickets).

## 7. S.M.A.R.T. and drive health

- Internal drives: IOKit `IOATASMARTInterface` (SATA) / NVMe SMART via `IONVMeSMARTInterface` (private-ish; `smartmontools` on macOS uses it). External USB drives: usually **no** SMART passthrough on macOS (no SAT support in the OS driver) — `smartmontools` needs a kext for that, which you should not ship. Design for "SMART when available, otherwise a quick read-error/latency probe".
- Health policy: if `Reallocated_Sector_Ct`/`Pending` > 0, NVMe `Critical Warning` set, or the probe sees latency spikes/read errors → force the "Image first" path in the GUI (skippable with a warning).

## 8. Reading through the OS vs. raw

Two source kinds must coexist:
1. **Raw block source** (`/dev/rdiskN`, images) — needed for carving, lost partitions, deleted metadata.
2. **Mounted-volume source** — walk the live filesystem via POSIX for Trash, snapshots, and as a fallback when raw access is refused. Also how "recover from FileVault volume" works when raw decrypted reads turn out to be unavailable (see §12 proof 3).

## 9. Distribution & signing

- Hardened runtime + notarization for the `.app`; the privileged helper must be embedded with `SMPrivilegedExecutables`-style requirements (modern: `SMAppService` + `LaunchDaemons/*.plist` in the bundle, `BundleProgram` key). No entitlement is required to read block devices — only root.
- App Sandbox is **incompatible** with raw device access → not a Mac App Store product. Distribute directly (+ Sparkle for updates) and via Homebrew cask/formula.
- Universal binary (arm64 + x86_64). Rust: `cargo build --target aarch64-apple-darwin --target x86_64-apple-darwin` + `lipo`.
- Minimum OS: 13 Ventura (for `SMAppService`); CLI can target lower.

## 10. Recovery-destination rules

- Refuse destination on the same physical disk as the source (compare IOKit whole-disk BSD names, not volume names). Also refuse if destination is inside a mounted image whose backing file is on the source disk.
- Warn when destination free space < selected size × 1.1.
- Preserve xattrs/resource forks/timestamps when recovered from HFS+/APFS metadata (`setattrlist`, `com.apple.ResourceFork` xattr).

## 11. Sources

- [Stellar: How Apple's T2 chip impacts Mac data recovery](https://www.stellarinfo.com/article/t2-chip-data-recovery.php)
- [CleverFiles: Target Disk Mode / Apple Silicon recovery guide](https://www.cleverfiles.com/help/data-recovery-target-disk-mode.html)
- [Wondershare: Mac system drive not visible to recovery software](https://recoverit.wondershare.com/faq/mac-system-drive-not-visible-recovery-software.html)
- [F-Response: Apple OSX and Full Disk Access](https://f-response.com/blog/apple_full_disk_access)
- [Hetman: APFS data recovery algorithm and structure](https://hetmanrecovery.com/recovery_news/data-recovery-algorithmfile-system-apfs-and-its-structure.htm)
- [Eclectic Light Co.: APFS containers and volumes](https://eclecticlight.co/2024/04/02/apfs-containers-and-volumes/)
- Apple Platform Security Guide — Volume encryption with FileVault / Secure Enclave (support.apple.com/guide/security)

## 12. Phase 0 verification results (M0)

Recorded during Phase 0 (build guide Part 4 step F). Test host, as enumerated by
`reclaim list`:

- **Machine:** `Mac17,6`, Apple **M5 Max**, macOS **26.6.2** (build `25G83`).
- **SIP:** enabled. **FileVault:** **On**. **Internal:** 2 TB `APPLE SSD AP2048Z`
  (`disk0`), NVMe/Apple Fabric, `TRIM Support: Yes`.
- **Device map:** `disk0` (whole, GPT) → `disk0s1` Apple_APFS_ISC, `disk0s2`
  Apple_APFS (physical store), `disk0s3` Apple_APFS_Recovery. Synthesized APFS
  containers: `disk1` (iSCPreboot/xART/Hardware/Recovery), `disk2`
  (Recovery/Update), **`disk3`** (the boot container) → `disk3s1` *Macintosh HD*
  (System, sealed), `disk3s2` Preboot, `disk3s3` Recovery, **`disk3s5` *Data***
  (`/System/Volumes/Data`), `disk3s6` VM.
- **No external sacrificial device was attached at Phase 0 build time.**

### The two access gates (verified empirically 2026-09-12)

Raw internal devices are guarded by **two independent gates**, and conflating
them caused the first proof run to misreport. Both were confirmed live on this
host:

- **DAC (Unix permissions).** `/dev/rdisk0`, `/dev/rdisk3`, `/dev/rdisk3s5` are
  `crw-r----- root:operator`. Opening them needs **root** *or* membership in the
  **`operator`** group. A plain non-root, non-operator process gets `EACCES`
  ("Permission denied").
- **TCC (Full Disk Access).** The boot / container / Data devices are
  TCC-protected. FDA is attributed to the shell's **responsible process** and is
  **uid-independent — root does not confer or inherit it**. On this host FDA is
  already granted and effective for the Claude Code helper bundle
  `com.anthropic.claude-code` (verified: this non-sudo shell reads
  `~/Library/Messages/chat.db` and the system `TCC.db`, which shows
  `kTCCServiceSystemPolicyAllFiles = 2` for that bundle). It is **not** granted
  to `/Applications/Claude.app` (`com.anthropic.claudefordesktop`) or Terminal.
- **The trap:** running the read under **`sudo`** satisfies DAC but re-parents the
  process and **drops the FDA attribution**, so a sudo'd read of a TCC-protected
  internal device returns `EPERM` ("Operation not permitted"). The decisive tell:
  under the same `sudo` run, a *user-attached* `hdiutil` image node
  (`/dev/rdisk4`, not TCC-protected) reads fine while `rdisk0/rdisk3/rdisk3s5` all
  fail. **The reliable path is NON-sudo + operator group** (DAC via operator, TCC
  via the already-effective helper FDA):

  ```
  sudo dseditgroup -o edit -a "$USER" -t user operator   # one-time
  # quit & relaunch Claude Code (group membership applies to new sessions), then:
  scripts/platform-proofs.sh                             # NO sudo
  ```

  Product implication for later phases: a user in `operator` with FDA can scan
  raw devices **without sudo**; and `sudo reclaim scan <internal>` may hit the TCC
  wall on the boot disk (but not on external/attached media, which is the primary
  camera/SD use case). `reclaim doctor` should check operator membership + FDA
  effectiveness, not just root.

### Device names for the next phases (resolve at runtime, never hardcode)
Synthesized disk numbers are dynamic (the attached test image already took
`disk4`). Resolve via `diskutil info -plist /System/Volumes/Data` (Data volume +
`APFSContainerReference`) and its physical store. On this host today:
- Boot whole internal disk: **`disk0`** (`/dev/rdisk0`), physical store **`disk0s2`**.
- Boot APFS container: **`disk3`** (`/dev/rdisk3`) — `NXSB` lives on the container /
  physical store, **not** the volume node.
- **User-data recovery target:** Data volume **`disk3s5`** (`/dev/rdisk3s5`),
  `Encryption=true, FileVault=true`, mount `/System/Volumes/Data`. `APSB` lives here.
- Golden test images (built, not committed): `testdata/build/*.img` +
  `*.groundtruth.json` — scannable via `reclaim info <img>` (no root).

### Proof outcomes (run 2026-09-12; re-run to close the deferred rows)

| # | Proof | Result |
|---|-------|--------|
| 1 | Raw read of a `/dev/rdiskN` device works | **PASS.** A user-attached `hdiutil` image node `/dev/rdisk4` reads correctly — both under `sudo` and non-sudo (the node is user-owned, not TCC-protected). The `RawDevice` code path is proven against a real raw device. External-device arm: pending any external disk (read-only, **not** erasable). |
| 2 | Raw read of the boot physical store; the FDA/permission gate | **PARTIAL / diagnosed.** Reads of `/dev/rdisk0` failed — root cause is the DAC-vs-TCC/`sudo`-drops-FDA trap above, **not** a missing FDA grant. To turn this into a clean PASS: operator group + non-sudo (script now classifies `EACCES` DAC vs `EPERM` TCC). **Deferred (needs Ryker: one-time operator add + app relaunch).** |
| 3 | Does the FileVault-unlocked Data volume return decrypted APFS blocks? | **OPEN (genuine unknown).** Node identified (`/dev/rdisk3s5`, container `/dev/rdisk3`, store `/dev/rdisk0s2`). Data volume is `Encryption=true`. On Apple Silicon the SEP decrypts transparently when booted+unlocked, so a successful read is *expected* to show `NXSB`/`APSB`; a high-entropy result would mean ciphertext. Not yet observed (read was DAC-blocked). The detection method is validated: the unencrypted APFS golden image shows 5× `NXSB`, 66× `APSB`. **Deferred (same operator+non-sudo path).** |
| 4 | TRIM timing (delete → wait → do extents read zeros?) | **Capability PASS; behavioral test deferred.** `system_profiler` reports `TRIM Support: Yes` for the internal SSD. The internal erasure-timing measurement is **not reliable** on the FileVault APFS boot volume (`F_LOG2PHYS` extent→raw-offset mapping is unusable on the encrypted, COW, snapshot-capable volume — verified: it returns garbage). The prior run's "reads ZEROS → TRIM happened" was a **false positive** (the raw read was denied → 0 bytes → miscounted as zeros) and is retracted. A clean behavioral test needs an **unencrypted external device** (§3 already establishes external USB/SD bridges usually do *not* TRIM). |

### Manual items (Ryker) — none block Phase 1
1. **Close proofs 2 & 3 (no media needed):** `sudo dseditgroup -o edit -a "$USER" -t user operator`, then **quit & relaunch Claude Code** (or use a fresh login shell), then run `scripts/platform-proofs.sh` **without sudo**. Record whether proof 3 shows DECRYPTED APFS or ciphertext.
2. **External TRIM behavioral test (optional):** any external disk with ~100 MB free (need not be erasable) — `scripts/platform-proofs.sh --ext-dev diskN --ext-mount /Volumes/<NAME>`. Only a whole-device wipe test would need a truly erasable drive.
3. Paste the resulting `testdata/build/platform-proofs.txt` block here.
