# 03 — Architecture

## 1. Layer diagram

```
┌──────────────────────────────────────────────────────────────────────┐
│  Presentation        reclaim (CLI, Rust)     Reclaim.app (SwiftUI)    │
│                            │                       │ UniFFI / C ABI   │
├────────────────────────────┴───────────────────────┴─────────────────┤
│  Session & Orchestration   reclaim-session                           │
│     scan planner · progress/events · checkpointing · result store    │
├──────────────────────────────────────────────────────────────────────┤
│  Engines                                                              │
│   reclaim-meta (FS parsers → deleted entries)                         │
│   reclaim-carve (signatures, validators, reassembly)                 │
│   reclaim-structs (lost partition / volume hunting)                  │
│   reclaim-raid (virtual arrays)        reclaim-crypto (unlock layers) │
├──────────────────────────────────────────────────────────────────────┤
│  Filesystem crates       fs-apfs  fs-hfs  fs-ntfs  fs-fat  fs-exfat   │
│                          fs-ext   fs-xfs  fs-btrfs fs-ufs  fs-iso ... │
│                          (all implement the `FileSystem` trait)      │
├──────────────────────────────────────────────────────────────────────┤
│  Block layer             reclaim-block: BlockSource trait, offset     │
│                          views, caching, bad-block map, imaging      │
├──────────────────────────────────────────────────────────────────────┤
│  Platform                reclaim-platform-macos: IOKit enumeration,  │
│                          /dev/rdisk open, DiskArbitration, S.M.A.R.T.,│
│                          privileged helper. (linux/windows later)     │
└──────────────────────────────────────────────────────────────────────┘
```

Everything above the block layer is platform-independent and testable against image files on any OS — this is what makes CI practical.

## 2. Core abstractions

### 2.1 `BlockSource`

```rust
pub trait BlockSource: Send + Sync {
    fn len(&self) -> u64;                       // bytes
    fn sector_size(&self) -> u32;               // logical
    fn physical_sector_size(&self) -> u32;
    /// Read exactly buf.len() bytes at `offset`. Returns per-sector status so
    /// callers can continue past unreadable sectors.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult;
    fn id(&self) -> SourceId;                   // stable across sessions
}
```

- **No write method exists on the trait.** The imager writes to a *destination* `File`, never to a `BlockSource`.
- Implementations: `RawDevice` (macOS `/dev/rdiskN`), `ImageFile` (raw/DMG/VMDK/E01/...), `OffsetView` (partition window), `RaidView`, `DecryptView`, `MappedImage` (image + bad-block map; unread regions return `ReadStatus::Unread`).
- `ReadResult` carries a bitmap of good/bad sectors for the request, so a carver can treat a bad sector as "unknown bytes" instead of aborting a file.
- A shared `ReadAheadCache` (LRU by 1 MiB chunk) sits on top so metadata walkers doing random reads don't thrash the device.

### 2.2 `FileSystem` trait (metadata engine)

```rust
pub trait FileSystem {
    fn probe(src: &dyn BlockSource) -> Option<Probe>;       // confidence 0..1, block size, label, uuid
    fn open(src: Arc<dyn BlockSource>, probe: Probe) -> Result<Self>;
    fn block_size(&self) -> u32;
    fn allocation_bitmap(&self) -> Option<Bitmap>;          // used/unused blocks (feeds carver "unallocated only")
    fn walk(&self, sink: &mut dyn EntrySink, opts: WalkOpts) -> Result<()>;  // live + deleted entries
    fn read_extents(&self, entry: &Entry) -> Result<ExtentList>;
    fn snapshots(&self) -> Vec<SnapshotRef>;                // APFS, Btrfs, ZFS
}
```

An `Entry` is: id, parent id, name (UTF-8 + raw bytes), kind, size, timestamps, extents, `state: Live | Deleted | Orphaned | Historical(snapshot/checkpoint)`, `confidence`, and FS-specific attrs (xattrs, resource fork, compression flag).

### 2.3 Carving engine

```
read stream (sequential, large blocks)
   → signature matcher (Aho-Corasick over all header patterns, aligned to
     candidate block sizes)
   → per-format Validator::inspect(header bytes) → accept/reject + expected size
   → Extractor strategy:
        FixedSize | HeaderDeclaredSize | FooterSearch(max_len) |
        StructuralWalk (parse chunks/atoms until end) | Statistical (text)
   → FragmentHints (for formats that support reassembly, emit candidate
     fragments instead of finalizing)
   → Result: CarvedFile { offset, len, format, validity: Full|Truncated|Suspect,
              embedded_name, embedded_timestamp, thumb_offset }
```

Design rules:
- Signatures live in **data files** (TOML, doc 05) *plus* optional Rust validators registered by format id. Adding a simple format never requires code.
- The matcher runs on block boundaries first (fast path), then on every byte within blocks flagged as "unallocated & non-zero & non-uniform entropy" (slow path) — PhotoRec's "brute force" equivalent, opt-in.
- Block size comes from the filesystem probe when available; otherwise inferred from the offsets of the first ~10 validated headers (GCD of offsets), as PhotoRec does.
- The carver consults the FS allocation bitmap when present so it can label results *unallocated* (likely deleted) vs *allocated* (a live file — usually filtered out unless the FS is damaged).

### 2.4 Lost-structure engine

Scans for anchors: MBR/GPT headers (primary & backup), APFS `NXSB` container superblocks & checkpoint descriptors, APFS `APSB` volume superblocks, HFS+ volume headers (`H+`/`HX` at offset 1024, plus alternate VH at end-1024), NTFS boot sectors (`NTFS    ` OEM ID) and `$MFT` records (`FILE0`), FAT BPBs and backup boot sectors, exFAT `EXFAT   `, ext superblocks (magic `0xEF53` at +0x438 and backup group copies), XFS `XFSB`, Btrfs `_BHRfS_M` at 64 KiB, ZFS uberblocks, UFS magic, ISO9660 `CD001`. Each hit yields a `ProposedVolume { start, len, fs, confidence, evidence }`; the planner can "adopt" one and hand it to the metadata engine as an `OffsetView`.

### 2.5 Session & result store

- One session = one SQLite file: `sources`, `scan_runs`, `entries`, `carved`, `fragments`, `proposed_volumes`, `progress`, `events`.
- Progress is an LBA cursor per engine pass + a compact set of "already-emitted" hashes; checkpoint every 5 s or 1 GiB.
- Result IDs are deterministic: `blake3(source_id || engine || offset || len)`.
- Previews are rendered on demand from source extents, never stored (except a small thumbnail cache keyed by result id).

### 2.6 Events

`enum Event { Progress{pass, lba, total, rate}, EntryFound(EntryId), VolumeProposed, ReadError{lba,len}, Warning(String), PassComplete, Done }` sent over a channel; CLI prints, GUI binds, `--json-events` writes NDJSON.

## 3. Concurrency model

- **One sequential reader** per device (failing drives hate random I/O; SSDs don't care). The reader pushes 4–16 MiB chunks into a bounded channel.
- **Fan-out consumers**: metadata engine (random reads via the cache, throttled), carver workers (N = cores, each owns a chunk, overlap handling by carrying the last `max_header_len` bytes of the previous chunk), structure hunter.
- Back-pressure via bounded channels; results written to SQLite by a single writer task in batched transactions.
- Async is unnecessary; use `std::thread` + `crossbeam-channel`. `rayon` for in-chunk parallel signature matching if profiling justifies it.

## 4. Crate layout (Cargo workspace)

```
reclaim/
  Cargo.toml                 (workspace)
  crates/
    reclaim-block/           BlockSource, views, cache, imaging, bad-block map
    reclaim-platform-macos/  IOKit/DiskArbitration enumeration, raw open, SMART
    reclaim-platform-linux/  (later) /sys/block, udev
    reclaim-fs-core/         FileSystem trait, Entry, Extent, Bitmap
    fs-apfs/ fs-hfs/ fs-ntfs/ fs-fat/ fs-exfat/ fs-ext/ fs-xfs/ fs-btrfs/ fs-ufs/ fs-iso/
    reclaim-part/            MBR, GPT, APM, LDM, LVM2, mdadm superblocks, Core Storage
    reclaim-carve/           matcher, validators, extractors, reassembly
    reclaim-sigs/            signature TOML catalog + build.rs codegen
    reclaim-structs/         lost volume hunting
    reclaim-raid/            virtual RAID, parameter inference
    reclaim-crypto/          decrypt views (APFS unlocked via OS, BitLocker, LUKS)
    reclaim-session/         planner, SQLite store, events, checkpoints
    reclaim-preview/         image decode (image-rs, libheif via ffi), pdf (pdfium-render), video frame (ffmpeg ffi, optional)
    reclaim-report/          JSON/HTML/CSV
    reclaim-ffi/             UniFFI bindings for Swift
  apps/
    reclaim-cli/
    Reclaim.app/             Xcode project (SwiftUI), PrivilegedHelper target
  testdata/                  generated image recipes (doc 09), NOT binaries
```

Recommended crates to evaluate (verify maintenance status before depending): `gpt`/`gptman` (GPT), `mbrman`, `ntfs` (by ColinFinck — mature, no_std), `fatfs`, `ext4-view`, `aho-corasick`, `memchr`, `rusqlite` (bundled), `blake3`, `zstd`, `image`, `kamadak-exif`, `mp4parse`, `symphonia` (audio), `pdfium-render`, `uniffi`, `io-kit-sys`/`core-foundation`. APFS, HFS+, XFS, Btrfs, UFS will most likely need in-house parsers written from the public specs (Apple's *Apple File System Reference* PDF; Apple TN1150 for HFS+; kernel docs for the Linux FSs).

## 5. Data flow for a typical "Deep scan SD card" run

1. GUI asks helper for device list → user picks `disk4` (exFAT, 64 GB).
2. Planner probes partition scheme (MBR) and FS (exFAT, cluster 128 KiB).
3. S.M.A.R.T. not available on USB readers → skip; read-speed test 1 s → healthy → **no forced imaging**, but UI offers it.
4. Quick pass: exFAT walker enumerates directory entries incl. deleted (entry type with in-use bit clear), emits entries with names and clusters; allocation bitmap loaded.
5. Deep pass: sequential read; carver aligned to 128 KiB cluster boundaries on unallocated clusters first; JPEG/HEIC/MP4/MOV/RAW validators; MP4 fragments recorded for later reassembly.
6. Merge: carved JPEG at cluster 12345 matches extents of deleted `DSC_0412.JPG` → collapsed into the named entry with `confidence: high`.
7. Results stream to UI; thumbnails decode from the card on demand.
8. User selects all photos → destination check (not disk4) → recover, verify, report.

## 6. Error handling philosophy

- Parsers return `Result<_, FsError>`; **never panic on data**. Corruption is the normal case.
- A read error is a value, not an exception: `ReadStatus::Bad` sectors become zero-filled with a flag; results touching them are marked `Suspect`.
- Engines are independent: a crash (panic caught at thread boundary) in one FS parser is logged as a warning and the other engines continue.

## 7. Security model

- Raw reads need root. GUI: a privileged helper (`SMAppService.daemon`) exposes an XPC API limited to *open-device-readonly* returning a file descriptor; the unprivileged app does all parsing. CLI: runs under `sudo`, immediately drops to opening the device and keeps running as root (or, better, opens the fd then `setuid` back to the invoking user).
- Destination writes happen in the unprivileged process.
- No network access in the core. Updates (Sparkle) only in the GUI.
