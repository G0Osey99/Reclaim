# Phase 6 — Benchmark, hardware QA, docs, release 1.0.0

- **Date:** 2026-09-12
- **Branch:** `dev/v1` (off `main`); the `p6:` commits.
- **Toolchain:** rustc/cargo **1.98.1** (pinned); `cargo-about` 0.9.2 (THIRD_PARTY),
  `clap_mangen`/`clap_complete` (CLI docs, feature `docgen`), PhotoRec 7.x
  (benchmark competitor). No paid Developer ID / EdDSA key on this host yet —
  those release steps are staged, not executed (Part 5.3, see Manual items).
- **Scope:** publish the numbers, cross-check the plan, finish user docs, stage
  the release, prepare the hardware runbook. **Bugs found in QA were fixed here;
  no features added.**

## Done

### A. Benchmark publication
- Extended `scripts/bench.sh` to score **every** golden image (Phases 0–4): the
  raw delete images, the ext2/ext4 deleted-file images (new ground-truth
  sidecars, deterministic content), the **container-wrapped** guests
  (`apfs-in-dmg`, `ntfs-in-vmdk`), and the **lost-structure** images
  (`exfat-zeroed-boot`, `hfs-zeroed-vh`). Added `bytes_read` (from the session
  log) and `partial_credit` columns.
- Wrote **[docs/benchmarks.md](../benchmarks.md)**: doc 09 §3 methodology verbatim,
  the per-image table (content_recall / named_recall / precision / partial_credit
  / time / bytes_read / peak_rss / PhotoRec), the environment, and the 512 GiB /
  2 M-file throughput + peak-RSS result. **content_recall ≥ PhotoRec on every
  image** (incl. `apfs-in-dmg`, where PhotoRec scores 0% — it can't read the DMG
  container).
- `scripts/gen-stress-image` builds the nightly 512 GiB sparse / 2 M-record image
  (doc 09 §2/§7); the scan found exactly **2,000,000** results with peak RSS far
  under the 2 GB cap (NFR-3) — the on-disk SQLite index keeps memory flat vs
  result count.

### B. Hardware QA runbook
- **[phase-6/hardware.md](phase-6/hardware.md)** — the doc 09 §6 checklist as a
  runbook + evidence log with exact commands and PASS/FAIL slots, to be executed
  **with Ryker** (sudo/FDA/FileVault, physical cards, hot-unplug, a second
  Mac/VM). Nothing is pre-filled PASS. The Recovery-Terminal item also closes the
  "recovery-mode.md executed on hardware" requirement.

### C. Plan cross-check
- **[phase-6/verification.md](phase-6/verification.md)** — every doc 02 FR/NFR id
  and every doc 01 §2 feature marked PASS / PASS·HW / SHIP / ROUND-2 / N/A with
  evidence. Every round-1 *code* item is PASS; the three QA bugs (below) were
  fixed here.

### D. Docs + legal
- **README.md** (install, 60-second CLI + app tutorial, the doc 11 §1 safety
  promises, the doc 06 §3 chances-by-media table), **docs/cli.md** (generated from
  clap via `scripts/gen-cli-docs.sh` — man pages + bash/zsh/fish completions too),
  **CONTRIBUTING.md** (DCO + clean-room signature rules, Part 3.4),
  **SECURITY.md**, **THIRD_PARTY.md** (`cargo about`, 198 crates, all permissive),
  **docs/EULA.md** + a first-run notice wired into the app onboarding (doc 11 §5),
  **CHANGELOG.md** `[1.0.0]`, **docs/sparkle.md**. All versions bumped to **1.0.0**
  (workspace, app Info.plist, helper, `build-app.sh`).

### E. Release (staged — not published)
- **scripts/release.sh** builds the CLI tarballs (aarch64 / x86_64 / universal
  darwin; x86_64 linux via cross toolchain or CI), the universal app + DMG (via
  `build-app.sh`), codesigns + notarizes the CLI + DMG **when a Developer ID +
  notary profile exist** (else ad-hoc with a clear note), writes `SHA256SUMS`, and
  generates the Sparkle **appcast.xml** (signed when the EdDSA key is present).
- **Sparkle**: appcast generation + `SUFeedURL`/`SUPublicEDKey` in Info.plist +
  the full integration procedure in **docs/sparkle.md**. The framework SPM wiring
  and the EdDSA key are Ryker/Xcode-gated (Part 5.3).
- **Homebrew tap** staged at `packaging/homebrew-reclaim/` (Formula = source
  build; Cask = notarized DMG) with `update.sh` to fill version/SHA after publish.

## Bugs found and fixed (QA)
1. **FAT/exFAT single-cluster validity** (`fs-fat`, `fs-exfat`): a deleted file
   that fits in a single cluster was marked `Suspect` via the contiguous
   assumption. A one-cluster file *is* contiguous — now `Full`. Over-conservative
   on real large-cluster camera cards; matched across both engines.
2. **Carved↔named over-carve merge** (`reclaim-session::store`): the merge only
   collapsed a carve whose `(offset,len)` exactly matched a named entry, so a carve
   that started at a named file's offset but over-ran its true end survived as a
   `Truncated` duplicate. Now collapses on the **start offset** (a physical byte
   hosts one file's start), removing the duplicate (FR-SCAN-6). Raised ext
   precision 90% → 100%.
3. **Benchmark precision metric** (`bench_score.py`): reported a matches-only ratio
   that counted every correctly-recovered real file (macOS `._*` AppleDouble,
   `.fseventsd/*`) as an error. Now computes precision per doc 09 §3
   (matches + valid_unknown / total) from the recover manifest's validity.
4. **`bench.sh` render crash** (`named_ok` undefined) and stale CLI `--help` text
   ("Phase 2 — no engines yet", "no-op until Phase 2") corrected.

## Gates
- `cargo ci` — **green** (fmt; clippy `--workspace --all-targets --all-features
  -D warnings`, incl. the new `docgen` bin; `cargo test --workspace`; `cargo deny`
  with `clap_mangen`/`clap_complete`/`roff` added, all permissive;
  `check-readonly`; `check-recovery-libs`). _(re-run on the final commit; see the
  cargo ci note at the bottom.)_
- **Benchmarks** — content_recall ≥ PhotoRec on every image; named_recall ≥ 0.95
  on exFAT/FAT/NTFS/HFS+ and ≥ 0.90 on APFS (overwrite-limit case documented);
  precision 1.0 on validity-`full` results everywhere (the FAT `._*` `suspect`
  residual is explained in docs/benchmarks.md, not softened). Peak RSS ≤ 2 GB on
  the 512 GiB / 2 M-file image.
- **hardware.md / verification.md** — verification.md complete (no code FAIL);
  hardware.md is the pending runbook for the Ryker session.

## Manual items (Ryker) — the ship gates
1. **Paid Apple Developer ID** (Part 5.3): membership + a *Developer ID
   Application* cert in the login keychain; `xcrun notarytool store-credentials
   reclaim-notary …`; `export DEVELOPMENT_TEAM=<TEAMID>`. Then `scripts/release.sh
   1.0.0` signs + notarizes the DMG and CLI and staples the DMG.
2. **Sparkle EdDSA key** (Part 5.3): `generate_keys` → put the public key in
   `Info.plist:SUPublicEDKey`; keep the private key in the keychain; re-run
   `release.sh` so `sign_update` signs the appcast (docs/sparkle.md). Add the
   Sparkle SPM dependency in Xcode (CLT can't resolve the binary xcframework).
3. **Run the hardware checklist** (phase-6/hardware.md) with sudo/FDA/FileVault,
   the sacrificial cards, a bad-sector drive, and a second Mac/VM.
4. **Publish (only on "ship"):** open PR `dev/v1 → main`; when `cargo ci` is green
   on `main`, merge, `git tag v1.0.0`, `gh release create v1.0.0` with the
   `dist/release/1.0.0/*` artifacts + SHA256SUMS + a benchmark summary; create the
   `homebrew-reclaim` repo from `packaging/homebrew-reclaim/`; run
   `packaging/homebrew-reclaim/update.sh 1.0.0`; verify `brew install
   --build-from-source G0Osey99/reclaim/reclaim` and the cask on a clean Mac.

## Release URLs & checksums
- **Pending publish** (Manual item 4). `scripts/release.sh 1.0.0` writes
  `dist/release/1.0.0/SHA256SUMS`; paste it and the release URL here once shipped.
  Nothing has been published: no PR opened, no tag pushed, no release created.

## Round-2 backlog
- **Fragment reassembly** — JPEG (multi-scan) and MP4/MOV (interleaved chunk)
  reconstruction; raise `partial_credit` above `content_recall`.
- **RAID** — 0/1/5/6/10 auto-detect + manual builder (app has the disabled menu).
- **Own-crypto unlock** — BitLocker / LUKS / VeraCrypt / FileVault from
  user-supplied keys (round 1 reads OS-unlocked volumes only).
- **ext/XFS/Btrfs/F2FS/ZFS depth** — full metadata walks beyond detect-and-carve;
  native UDF directory walk.
- **Volume managers** — Core Storage / Fusion LV assembly, LVM2, mdadm, LDM,
  Storage Spaces, SHR.
- **Imaging** — entropy map; AFF4 and E01 *write*; VMDK streamOptimized;
  large multi-node sparseimage band tables.
- **Deletion-journal agent** (Recovery-Vault equivalent).
- **Windows / Linux GUI**; single-pass fold of the structure sweep into the carve
  read; deeper IOKit SMART attribute FFI.

## Next phase needs
- Benchmarks regenerate with `scripts/bench.sh` (all golden images) +
  `scripts/gen-stress-image` (nightly 512 GiB). CLI docs: `scripts/gen-cli-docs.sh`.
  Licenses: `scripts/gen-third-party.sh`. Release: `scripts/release.sh <ver>`.
- The `docgen` feature (`crates/reclaim-cli`, `src/docgen.rs`) is the single source
  for man pages / completions / cli.md — never edit those by hand.
