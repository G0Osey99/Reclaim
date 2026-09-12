# 08 — GUI Specification (Reclaim.app)

Native SwiftUI (macOS 13+), thin client over the Rust core via UniFFI. The GUI owns: privilege escalation (helper), onboarding/permissions, source selection, live results browsing with previews, recovery, imaging, and reports. It contains **no recovery logic**.

## 1. Information architecture

```
Sidebar                         Main
────────────────                ────────────────────────────────────────
Sources                          [Source detail / scan plan]
  ▸ Internal (disk0)
      Macintosh HD - Data        Health · FileVault · TRIM note
  ▸ External (disk4)             Quick/Deep/Full  ▶ Scan   ⧉ Image first
      NO NAME (exFAT)
  ▸ Images & sessions
      sd64.img (session 12 Sep)
Tools
  Byte-to-byte image
  Lost volumes
  RAID builder
  Snapshot browser
  S.M.A.R.T.
Sessions (recent)
```

Toolbar: Scan ▶ / Pause ⏸ / Stop ■, filter field, view (grid/list/tree), Recover… button with selection size.

## 2. Screens & flows

### 2.1 First launch / onboarding
1. Explain read-only guarantee in one sentence.
2. Install helper (`SMAppService` prompt) — status pill.
3. Full Disk Access check (attempt raw read through helper) → if missing, button opens `x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles`; re-check on window focus.
4. "Stop using the drive you lost data on" tip card, dismissible.

### 2.2 Source detail
- Header: icon, name, model, size, bus, health chip (green/amber/red from SMART/probe).
- Partition map bar (proportional, colour by FS), click to select a partition.
- Warnings block: boot disk in use / TRIM SSD low chances / FileVault locked (Unlock… button runs `diskutil apfs unlockVolume` through helper) / read errors seen → **Image first** primary button replaces Scan when health is red.
- Scan configuration disclosure: engines, families, range, brute-force, block size (auto), threads.
- Estimated time from the planner.

### 2.3 Scanning / results (single screen — results appear live)
- Top: progress bar per engine pass, rate, ETA, found counters by family, read-error count. Pause/Resume/Stop. "Session auto-saved 12 s ago".
- Left filter rail: Family (Photos, Video, Audio, Documents, Archives, Other) with counts; Extension; Date range; Size; Recoverability (High/Medium/Low); Source engine (Named files / Reconstructed / Carved); "Deleted only" toggle; path search.
- Center: **Grid** (thumbnails, lazy decode from source, placeholder while scan reads) / **List** (name, path, size, date, chances, engine) / **Tree** (recovered folder hierarchy from metadata engine; carved files under "Reconstructed" pseudo-folders by type/date).
- Right inspector: preview (image zoomable; video first frame + scrubbable when full; PDF page 1; text; audio play; hex fallback), metadata table (EXIF/ID3/etc.), extents list and overwrite status, fragments if any, validity.
- Selection footer: N items · 3.2 GB · **Recover…**.

### 2.4 Recover sheet
- Destination picker (folder). Real-time validation: same-disk refusal (explain), space check.
- Options: preserve folder structure / flat by type; name collisions; include Suspect/Truncated (default on, badge in results); verify after copy.
- Progress with per-file failures list; final summary + "Open in Finder" + "Save report".

### 2.5 Byte-to-byte image tool
- Source → destination file; block size; passes; sparse/zstd; hash; live bad-sector map visualization (1-pixel-per-N-blocks strip: green/red/grey); pause/resume; "Scan this image" on completion.

### 2.6 Lost volumes
- List of proposals with evidence text ("HFS+ volume header at LBA 409640, alt header matches, size 499 GB"); Adopt → appears in sidebar as a virtual source.

### 2.7 RAID builder
- Drag members into order; level; chunk; parity rotation; "Detect" button; live sanity indicator (does an NTFS/ext4 superblock parse on the virtual array?).

### 2.8 Snapshot browser (no root needed)
- List local TM snapshots and APFS snapshots; diff vs. current; restore selected files to a destination.

### 2.9 Reports
- HTML report (self-contained, thumbnails optional) and JSON; CSV of results.

## 3. States & error handling

| State | UI |
|-------|----|
| Helper not installed / denied | Blocking card in sidebar with Install button; app still usable for images and snapshot browser |
| FDA missing | Amber banner; boot disk shows lock icon |
| Source unplugged mid-scan | Pause automatically, session saved, "Reconnect to resume" — match by SourceId on reappear |
| Read errors climbing | Toast suggesting Image tool; auto-switch when > threshold with user consent |
| Destination refused | Inline red text with reason; button disabled |
| Core panic in engine | Non-fatal toast "Reconstructed-file engine stopped (bug logged)"; other engines continue |

## 4. Visual / interaction guidelines
- System appearance (light/dark), SF Symbols, native controls; no custom chrome.
- Grid thumbnails 128–256 px, decoded off-main-thread with cancellation on scroll.
- Never block the UI on the core: all calls async through the FFI, results paged from SQLite (`LIMIT/OFFSET` by filter), counts via indexed queries.
- Keyboard: ⌘F filter, Space QuickLook-style preview, ⌘R recover, ⌘. stop.
- Accessibility: every thumbnail has a label (name/type/size), VoiceOver rotor for families, reduced-motion respected.

## 5. FFI surface (UniFFI)

```
namespace reclaim {
  sequence<Source> list_sources();
  SourceInfo probe(string source);
  SessionHandle open_session(string path);
  SessionHandle start_scan(string source, ScanOptions opts, EventSink sink);
  void pause(SessionHandle); void resume(SessionHandle); void stop(SessionHandle);
  ResultPage query(SessionHandle, ResultFilter f, u32 offset, u32 limit);
  bytes preview(SessionHandle, string result_id, PreviewKind kind, u32 max_px);
  RecoverHandle recover(SessionHandle, sequence<string> ids, RecoverOptions o, EventSink sink);
  ImageHandle image(string source, string dest, ImageOptions o, EventSink sink);
  ...
};
callback interface EventSink { void on_event(Event e); };
```
Helper XPC: `openDeviceReadOnly(bsdName) -> fd`, `unlockAPFS(volume, passphrase) -> Bool`, `smart(bsdName) -> SmartReport`. The fd is passed to the core in-process; the helper never parses anything.

## 6. Distribution
Direct download `.dmg`, Sparkle updates, Homebrew cask. Notarized. Version string shared with the CLI (same workspace version).
