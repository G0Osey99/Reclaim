# `reclaim` command-line reference

_Generated from the clap definition (`cargo run -p reclaim-cli --features docgen`, via `scripts/gen-cli-docs.sh`) — do not edit by hand. Version 1.0.0._

Every device open is `O_RDONLY`; sources are never written. See [docs/plan/07-cli-spec.md](plan/07-cli-spec.md) for the design spec and the exit-code contract below.

## Synopsis

```text
Read-only disk/file/photo recovery

Usage: reclaim [OPTIONS] <COMMAND>

Commands:
  list          Enumerate sources (devices, partitions, volumes)
  info          Show details for a source
  doctor        Permissions / root / Full Disk Access / SIP self-check
  scan          Deep-carve a source into a session
  results       Query/filter a session's results
  recover       Copy selected results out to a destination
  report        Write a JSON/HTML/CSV report
  preview       Write a result's bytes or embedded thumbnail
  image         Byte-for-byte image a source with a bad-block map
  verify-image  Verify an image against its map
  sigs          Signature catalog tools
  snapshots     List APFS snapshots + reachable checkpoints, or diff one against now
  volumes       Find lost/damaged volumes (FR-SCAN-4); `adopt N` yields a scannable source

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help

  -V, --version
          Print version
```

## Exit codes

| code | meaning |
|---|---|
| 0 | success |
| 1 | usage / bad arguments |
| 2 | permission / root / Full Disk Access problem (`reclaim doctor` explains) |
| 3 | completed with warnings (bad sectors, partial/truncated files) |
| 4 | interrupted — session is resumable (`scan --resume`) |
| 5 | refused for safety (same-device destination, or a writable mount) — override with `--allow-same-device-i-accept-data-loss` |
| 6 | source not found or vanished mid-scan |
| 7 | internal error (bug) |

## Commands

### `reclaim list`

Enumerate sources (devices, partitions, volumes)

```text
Enumerate sources (devices, partitions, volumes)

Usage: reclaim list [OPTIONS]

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

### `reclaim info`

Show details for a source

```text
Show details for a source

Usage: reclaim info [OPTIONS] <SOURCE>

Arguments:
  <SOURCE>
          `diskN`, `/dev/rdiskN`, or an image file path

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

### `reclaim doctor`

Permissions / root / Full Disk Access / SIP self-check

```text
Permissions / root / Full Disk Access / SIP self-check

Usage: reclaim doctor [OPTIONS]

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

### `reclaim scan`

Deep-carve a source into a session

```text
Deep-carve a source into a session

Usage: reclaim scan [OPTIONS] <SOURCE>

Arguments:
  <SOURCE>
          Source to scan

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --quick
          Metadata pass only (list live + deleted entries by name; skip carving)

      --deep
          Signature-carving pass only (skip the metadata pass)

      --plain
          No colour / box-drawing / progress bars

      --families <FAMILIES>
          Restrict to families (comma-separated)

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --sigs <SIG_IDS>
          Restrict to signature ids (comma-separated)

      --no-interactive
          Never prompt; fail instead

      --range <RANGE>
          Byte range to scan, e.g. `0-32GiB`

      --session <PATH>
          Session file/dir to use or create

      --unallocated-only
          Restrict carved results to unallocated space (needs the FS free-space bitmap)

      --block-size <BLOCK_SIZE>
          Block size: `auto` or a byte count
          
          [default: auto]

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --brute-force
          Byte-granular matching in non-uniform blocks

      --prove-readonly
          Log every device open() with its flags (read-only audit)

      --no-keep-corrupted
          Hide Truncated/Suspect results

      --max-file-size <MAX_FILE_SIZE>
          Cap on any single carved file
          
          [default: 4GiB]

      --threads <THREADS>
          Worker threads (reserved; the scan is a single sequential reader)

      --checkpoint <CHECKPOINT>
          Checkpoint interval, e.g. `5s`
          
          [default: 5s]

      --resume
          Continue the session's interrupted scan

  -h, --help
          Print help
```

### `reclaim results`

Query/filter a session's results

```text
Query/filter a session's results

Usage: reclaim results [OPTIONS] [SESSION]

Arguments:
  [SESSION]
          Session directory (else --session / default)

Options:
      --family <FAMILY>
          Filter by family

      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --ext <EXTS>
          Filter by extension (comma-separated)

      --plain
          No colour / box-drawing / progress bars

      --min-size <MIN_SIZE>
          Minimum size, e.g. `100KiB`

  -v, --verbose...
          Increase verbosity (repeatable)

      --after <AFTER>
          Embedded date on/after `YYYY-MM-DD`

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --path <PATH>
          Glob on the synthesized path

      --min-score <MIN_SCORE>
          Minimum score

      --engine <ENGINE>
          Filter by engine

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --full-only
          Only Full results

      --prove-readonly
          Log every device open() with its flags (read-only audit)

      --deleted-only
          Only deleted/orphaned/historical named entries

      --sort <SORT>
          Sort: size|date|path|score|offset

      --limit <LIMIT>
          Row limit

      --format <FORMAT>
          Output: table|tree|json|csv|ids
          
          [default: table]

  -h, --help
          Print help
```

### `reclaim recover`

Copy selected results out to a destination

```text
Copy selected results out to a destination

Usage: reclaim recover [OPTIONS] <SESSION> <DEST>

Arguments:
  <SESSION>
          Session directory

  <DEST>
          Destination directory

Options:
      --ids <IDS>
          Read result ids from FILE (or `-` for stdin)

      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --all
          Recover everything

      --plain
          No colour / box-drawing / progress bars

      --family <FAMILY>
          Filter by family

  -v, --verbose...
          Increase verbosity (repeatable)

      --ext <EXTS>
          Filter by extension (comma-separated)

  -q, --quiet
          Suppress non-essential output

      --min-size <MIN_SIZE>
          Minimum size

      --no-interactive
          Never prompt; fail instead

      --full-only
          Only Full results

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --preserve-paths
          Recreate directory structure

      --flat
          Flatten into one directory

      --prove-readonly
          Log every device open() with its flags (read-only audit)

      --collision <COLLISION>
          Collision policy: rename|skip|overwrite
          
          [default: rename]

      --verify
          Hash + record recovered files (manifest always written)

      --no-fragments
          Do not reassemble fragments (reserved; round 1 carves contiguous only)

      --allow-same-device-i-accept-data-loss
          Allow a same-disk destination (accepts data-loss risk)

  -h, --help
          Print help
```

### `reclaim report`

Write a JSON/HTML/CSV report

```text
Write a JSON/HTML/CSV report

Usage: reclaim report [OPTIONS] <SESSION> <OUT>

Arguments:
  <SESSION>
          Session directory

  <OUT>
          Output file (format inferred from extension unless --format)

Options:
      --format <FORMAT>
          json|html|csv

      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

### `reclaim preview`

Write a result's bytes or embedded thumbnail

```text
Write a result's bytes or embedded thumbnail

Usage: reclaim preview [OPTIONS] <ID> [SESSION]

Arguments:
  <ID>
          Result id

  [SESSION]
          Session directory (else --session / default)

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --out <OUT>
          Output file (default stdout)

      --plain
          No colour / box-drawing / progress bars

      --thumb
          Prefer the embedded JPEG thumbnail

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

### `reclaim image`

Byte-for-byte image a source with a bad-block map

```text
Byte-for-byte image a source with a bad-block map

Usage: reclaim image [OPTIONS] <SOURCE> <OUT>

Arguments:
  <SOURCE>
          Source to image

  <OUT>
          Output image path

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --map <MAP>
          Sidecar map path (default OUT.reclaim-map)

      --block <BLOCK>
          Read block size

      --plain
          No colour / box-drawing / progress bars

      --retries <RETRIES>
          Retry count for bad regions
          
          [default: 3]

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --skip <SKIP>
          Skip stride after errors (reserved)

      --no-interactive
          Never prompt; fail instead

      --passes <PASSES>
          Number of passes (reserved; per-chunk retry is automatic)

      --reverse-pass
          Add a reverse retry pass

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --sparse
          Sparse output

      --prove-readonly
          Log every device open() with its flags (read-only audit)

      --zstd
          zstd-framed output

      --hash <HASH>
          Hash algorithm: blake3|sha256
          
          [default: blake3]

      --resume
          Resume from the map

  -h, --help
          Print help
```

### `reclaim verify-image`

Verify an image against its map

```text
Verify an image against its map

Usage: reclaim verify-image [OPTIONS] <IMAGE>

Arguments:
  <IMAGE>
          Image path

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --map <MAP>
          Sidecar map path (default IMG.reclaim-map)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

### `reclaim sigs`

Signature catalog tools

```text
Signature catalog tools

Usage: reclaim sigs [OPTIONS] <COMMAND>

Commands:
  list  List the signature catalog
  test  Identify a file against the catalog

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

### `reclaim snapshots`

List APFS snapshots + reachable checkpoints, or diff one against now

```text
List APFS snapshots + reachable checkpoints, or diff one against now

Usage: reclaim snapshots [OPTIONS] <SOURCE> [COMMAND]

Commands:
  diff  List files present at a snapshot/checkpoint but absent now

Arguments:
  <SOURCE>
          `diskN`, `/dev/rdiskN`, or an image file with an APFS container

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

### `reclaim volumes`

Find lost/damaged volumes (FR-SCAN-4); `adopt N` yields a scannable source

```text
Find lost/damaged volumes (FR-SCAN-4); `adopt N` yields a scannable source

Usage: reclaim volumes [OPTIONS] <SOURCE> [COMMAND]

Commands:
  adopt  Adopt proposal N as a `session:<dir>/volume/N` source for `scan`

Arguments:
  <SOURCE>
          `diskN`, `/dev/rdiskN`, or an image file (possibly a container)

Options:
      --json
          Emit machine-readable NDJSON on stdout (human text goes to stderr)

      --plain
          No colour / box-drawing / progress bars

  -v, --verbose...
          Increase verbosity (repeatable)

  -q, --quiet
          Suppress non-essential output

      --no-interactive
          Never prompt; fail instead

      --session <PATH>
          Session file/dir to use or create

      --log <FILE>
          Structured log file (reserved; scans always write log.ndjson in the session)

      --prove-readonly
          Log every device open() with its flags (read-only audit)

  -h, --help
          Print help
```

