# 07 — CLI Specification (`reclaim`)

Design principles: every GUI action has a CLI equivalent; machine-readable output (`--json`) is a first-class contract; sessions are files; nothing is interactive unless a TTY is present and `--no-interactive` is absent.

## 1. Command grammar

```
reclaim [GLOBAL OPTS] <COMMAND> [ARGS]

GLOBAL OPTS
  --json                 NDJSON events on stdout, human text on stderr
  --plain                no colour/box-drawing/progress bars
  -v/-vv/-q              verbosity
  --log <file>           structured log (always written to session dir too)
  --no-interactive       never prompt; fail instead
  --session <path>       session file/dir to use or create (default: ~/Library/Application Support/Reclaim/sessions/<auto>)

COMMANDS
  list                       enumerate sources
  info <SOURCE>              partition/FS/health details, proposed scan plan
  image <SOURCE> <OUT>       byte-to-byte image with bad-block map
  verify-image <IMG>         check map + hashes
  scan <SOURCE>              run engines, populate session
  results [SESSION]          query/filter results
  preview <RESULT-ID>        write preview (thumbnail/text/hex) to stdout or file
  recover [SESSION] <DEST>   copy selected results out
  volumes [SESSION]          show proposed lost volumes; adopt one
  raid build ...             define a virtual array from members
  snapshots <VOLUME>         list/diff APFS/Btrfs snapshots
  report [SESSION] <OUT>     HTML/JSON/CSV
  sigs list|test|add         signature catalog tools
  fs-check <SOURCE>          read-only structural sanity check with findings
  doctor                     permissions/root/FDA/notarization self-check
```

`SOURCE` grammar: `disk3`, `/dev/rdisk3s5`, `/path/image.dmg`, `image.E01`, `session:<id>/volume/<n>` (an adopted proposed volume), `raid:<name>`, `mounted:/Volumes/Foo` (POSIX walk mode).

## 2. Key commands in detail

### `list`
```
$ sudo reclaim list
DISK    SIZE     BUS    MODEL                       CONTENT        HEALTH
disk0   1.0 TB   NVMe   APPLE SSD AP1024Z           APFS container  ok (SMART)
 disk3  1.0 TB   —      (synthesized container)
  disk3s1  System (sealed)     APFS   mounted / (ro)
  disk3s5  Macintosh HD - Data APFS   mounted /System/Volumes/Data   FileVault: unlocked
disk4   64 GB    USB    SanDisk Extreme (SD reader)  MBR            n/a (USB)
  disk4s1  NO NAME             exFAT  mounted /Volumes/NO NAME
```
`--json` → array of `Source` objects with all fields from doc 03.

### `info <SOURCE>`
Probes scheme/FS/encryption, runs a 1-second read benchmark and SMART check, prints the **scan plan** the planner would use (engines, block size, estimated time) and warnings (source is boot disk, TRIM likely, FileVault locked, health).

### `image`
```
reclaim image disk4 ~/cards/sd64.img [--map ~/cards/sd64.map] [--block 1MiB]
        [--retries 3] [--skip 64KiB] [--passes 3] [--reverse-pass] [--sparse]
        [--zstd] [--hash sha256] [--resume]
```
Progress line: `pass 2/3  41.3%  212 MB/s  bad: 17 sectors  eta 3m12s`. Exit 0 = complete; 3 = complete with bad sectors; 4 = interrupted (resumable).

### `scan`
```
reclaim scan disk4 [--session S] [--engines meta,carve,structs] [--quick] [--deep]
        [--range 0-32GiB] [--unallocated-only] [--families image,video,raw]
        [--sigs image.jpeg,video.mp4] [--block-size auto|4096|131072]
        [--brute-force]       # byte-granular matching in non-zero unallocated blocks
        [--keep-corrupted]    # emit Truncated/Suspect results (default on; --no-keep-corrupted to hide)
        [--max-file-size 4GiB] [--threads N] [--checkpoint 5s]
        [--resume]            # continue the session's interrupted scan
```
Events (`--json`): `{"ev":"progress","pass":"carve","lba":...,"pct":41.3,"rate":...}`, `{"ev":"found","id":"…","engine":"meta","path":"DCIM/100CANON/IMG_0412.CR2","size":...,"score":92}`, `{"ev":"volume","start":...,"fs":"hfsplus","confidence":0.9}`, `{"ev":"readerror","lba":...}`, `{"ev":"done","found":12345,"elapsed":...}`.

### `results`
```
reclaim results [S] [--family image] [--ext jpg,cr2] [--min-size 100KiB] [--after 2026-01-01]
        [--path 'DCIM/*'] [--deleted-only] [--min-score 50] [--engine carve]
        [--sort size|date|path|score] [--limit N] [--format table|json|csv|ids]
```
Prints stable result ids; `--format ids` pipes into `recover --ids -`.

### `recover`
```
reclaim recover [S] ~/Recovered [--ids FILE|-] [--all] [--filters as results…]
        [--preserve-paths] [--flat] [--collision rename|skip|overwrite]
        [--verify] [--no-fragments] [--allow-same-device-i-accept-data-loss]
```
Writes `manifest.json` in DEST with per-file source offsets, hashes, validity. Exit 0 all ok; 3 some files partial/suspect; 5 destination refused.

### `volumes` / `raid`
```
reclaim volumes S                         # list proposals with evidence
reclaim volumes S adopt 2                 # creates source session:S/volume/2
reclaim raid build --name nas --level 5 --chunk 64KiB --order disk5,disk6,disk7 [--parity left-sym] [--missing 1]
reclaim raid detect disk5 disk6 disk7      # infer level/chunk/order from ext4/NTFS structures
```

### `snapshots`
```
reclaim snapshots disk3s5                  # list APFS snapshots + checkpoint xids reachable
reclaim snapshots disk3s5 diff --from com.apple.TimeMachine.2026-09-11-090000.local
```

### `doctor`
Checks: running as root? FDA granted (attempt boot-disk raw read)? SIP status (informational). Notarization/ codesign of self. Prints fix-it instructions.

## 3. Exit codes

| Code | Meaning |
|------|---------|
| 0 | success |
| 1 | usage error |
| 2 | permission/root/FDA problem (`doctor` explains) |
| 3 | completed with warnings (bad sectors, partial files) |
| 4 | interrupted, session resumable |
| 5 | refused for safety (same-device destination, source is writable-mounted and `--allow-mounted` absent) |
| 6 | source not found / vanished mid-scan |
| 7 | internal error (bug — please report with `--log`) |

## 4. Session file layout

```
<session>/
  session.sqlite      results, progress, events, config
  source.json         SourceId, size, sector sizes, hashes of first/last MiB (to re-identify)
  plan.json           engines & options
  log.ndjson
  thumbs/             small cache
```
Re-opening a session with a different source (hash mismatch) is refused unless `--force-source`.

## 5. Safety interlocks in the CLI

- Refuses to scan a mounted-writable volume unless `--allow-mounted` (reads are safe, but the user should stop writes; the flag is an acknowledgement).
- `recover` compares destination's whole-disk identity with the source's.
- All device opens are `O_RDONLY`; `reclaim` has a `--prove-readonly` debug flag that lists every open() with flags for auditing (used in tests).

## 6. Implementation notes

- `clap` derive for the grammar; `indicatif` progress; `serde_json` events; `comfy-table` for tables.
- Human output to stderr when `--json` so stdout stays pure NDJSON.
- Signals: SIGINT → checkpoint → exit 4. Second SIGINT → immediate exit (session may lose ≤ checkpoint interval).
- Man page + shell completions generated by `clap_mangen`/`clap_complete`.
