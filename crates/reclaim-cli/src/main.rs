//! `reclaim` — read-only disk / file / photo recovery CLI.
//!
//! Every device open is `O_RDONLY`; `--prove-readonly` prints the audited open
//! log so an auditor can confirm no source was ever opened writable
//! (docs/plan/07 §5.3). Writes happen only in the imager destination, the
//! session store, and the recover/report/preview destinations.

mod commands;
#[cfg(feature = "docgen")]
mod docgen;
mod exit;
mod source;
mod util;

use clap::{Parser, Subcommand};
use commands::recover::Collision;
use exit::{CmdError, Exit};
use reclaim_session::QueryFilter;
use std::path::PathBuf;
use std::process::ExitCode;

/// Read-only disk, file and photo recovery.
#[derive(Debug, Parser)]
#[command(
    name = "reclaim",
    version,
    about = "Read-only disk/file/photo recovery",
    disable_help_subcommand = true
)]
struct Cli {
    /// Emit machine-readable NDJSON on stdout (human text goes to stderr).
    #[arg(long, global = true)]
    json: bool,
    /// No colour / box-drawing / progress bars.
    #[arg(long, global = true)]
    plain: bool,
    /// Increase verbosity (repeatable).
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Suppress non-essential output.
    #[arg(short = 'q', long, global = true)]
    quiet: bool,
    /// Never prompt; fail instead.
    #[arg(long, global = true)]
    no_interactive: bool,
    /// Session file/dir to use or create.
    #[arg(long, global = true, value_name = "PATH")]
    session: Option<PathBuf>,
    /// Structured log file (reserved; scans always write log.ndjson in the session).
    #[arg(long, global = true, value_name = "FILE")]
    log: Option<PathBuf>,
    /// Log every device open() with its flags (read-only audit).
    #[arg(long = "prove-readonly", global = true)]
    prove_readonly: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Enumerate sources (devices, partitions, volumes).
    List,
    /// Show details for a source.
    Info {
        /// `diskN`, `/dev/rdiskN`, or an image file path.
        source: String,
    },
    /// Permissions / root / Full Disk Access / SIP self-check.
    Doctor,
    /// Deep-carve a source into a session.
    Scan {
        /// Source to scan.
        source: String,
        /// Metadata pass only (list live + deleted entries by name; skip carving).
        #[arg(long)]
        quick: bool,
        /// Signature-carving pass only (skip the metadata pass).
        #[arg(long)]
        deep: bool,
        /// Restrict to families (comma-separated).
        #[arg(long, value_delimiter = ',')]
        families: Vec<String>,
        /// Restrict to signature ids (comma-separated).
        #[arg(long = "sigs", value_delimiter = ',')]
        sig_ids: Vec<String>,
        /// Byte range to scan, e.g. `0-32GiB`.
        #[arg(long)]
        range: Option<String>,
        /// Restrict carved results to unallocated space (needs the FS free-space bitmap).
        #[arg(long = "unallocated-only")]
        unallocated_only: bool,
        /// Block size: `auto` or a byte count.
        #[arg(long = "block-size", default_value = "auto")]
        block_size: String,
        /// Byte-granular matching in non-uniform blocks.
        #[arg(long = "brute-force")]
        brute_force: bool,
        /// Hide Truncated/Suspect results.
        #[arg(long = "no-keep-corrupted")]
        no_keep_corrupted: bool,
        /// Cap on any single carved file.
        #[arg(long = "max-file-size", default_value = "4GiB")]
        max_file_size: String,
        /// Worker threads (reserved; the scan is a single sequential reader).
        #[arg(long)]
        threads: Option<usize>,
        /// Checkpoint interval, e.g. `5s`.
        #[arg(long, default_value = "5s")]
        checkpoint: String,
        /// Continue the session's interrupted scan.
        #[arg(long)]
        resume: bool,
    },
    /// Query/filter a session's results.
    Results {
        /// Session directory (else --session / default).
        session: Option<PathBuf>,
        /// Filter by family.
        #[arg(long)]
        family: Option<String>,
        /// Filter by extension (comma-separated).
        #[arg(long = "ext", value_delimiter = ',')]
        exts: Vec<String>,
        /// Minimum size, e.g. `100KiB`.
        #[arg(long = "min-size")]
        min_size: Option<String>,
        /// Embedded date on/after `YYYY-MM-DD`.
        #[arg(long)]
        after: Option<String>,
        /// Glob on the synthesized path.
        #[arg(long)]
        path: Option<String>,
        /// Minimum score.
        #[arg(long = "min-score")]
        min_score: Option<u8>,
        /// Filter by engine.
        #[arg(long)]
        engine: Option<String>,
        /// Only Full results.
        #[arg(long = "full-only")]
        full_only: bool,
        /// Only deleted/orphaned/historical named entries.
        #[arg(long = "deleted-only")]
        deleted_only: bool,
        /// Sort: size|date|path|score|offset.
        #[arg(long)]
        sort: Option<String>,
        /// Row limit.
        #[arg(long)]
        limit: Option<usize>,
        /// Output: table|tree|json|csv|ids.
        #[arg(long, default_value = "table")]
        format: String,
    },
    /// Copy selected results out to a destination.
    Recover {
        /// Session directory.
        session: PathBuf,
        /// Destination directory.
        dest: PathBuf,
        /// Read result ids from FILE (or `-` for stdin).
        #[arg(long)]
        ids: Option<String>,
        /// Recover everything.
        #[arg(long)]
        all: bool,
        /// Filter by family.
        #[arg(long)]
        family: Option<String>,
        /// Filter by extension (comma-separated).
        #[arg(long = "ext", value_delimiter = ',')]
        exts: Vec<String>,
        /// Minimum size.
        #[arg(long = "min-size")]
        min_size: Option<String>,
        /// Only Full results.
        #[arg(long = "full-only")]
        full_only: bool,
        /// Recreate directory structure.
        #[arg(long = "preserve-paths")]
        preserve_paths: bool,
        /// Flatten into one directory.
        #[arg(long)]
        flat: bool,
        /// Collision policy: rename|skip|overwrite.
        #[arg(long, default_value = "rename")]
        collision: String,
        /// Hash + record recovered files (manifest always written).
        #[arg(long)]
        verify: bool,
        /// Do not reassemble fragments (reserved; round 1 carves contiguous only).
        #[arg(long = "no-fragments")]
        no_fragments: bool,
        /// Allow a same-disk destination (accepts data-loss risk).
        #[arg(long = "allow-same-device-i-accept-data-loss")]
        allow_same_device: bool,
    },
    /// Write a JSON/HTML/CSV report.
    Report {
        /// Session directory.
        session: PathBuf,
        /// Output file (format inferred from extension unless --format).
        out: PathBuf,
        /// json|html|csv.
        #[arg(long)]
        format: Option<String>,
    },
    /// Write a result's bytes or embedded thumbnail.
    Preview {
        /// Result id.
        id: String,
        /// Session directory (else --session / default).
        session: Option<PathBuf>,
        /// Output file (default stdout).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Prefer the embedded JPEG thumbnail.
        #[arg(long)]
        thumb: bool,
    },
    /// Byte-for-byte image a source with a bad-block map.
    Image {
        /// Source to image.
        source: String,
        /// Output image path.
        out: PathBuf,
        /// Sidecar map path (default OUT.reclaim-map).
        #[arg(long)]
        map: Option<PathBuf>,
        /// Read block size.
        #[arg(long)]
        block: Option<String>,
        /// Retry count for bad regions.
        #[arg(long, default_value_t = 3)]
        retries: u32,
        /// Skip stride after errors (reserved).
        #[arg(long)]
        skip: Option<String>,
        /// Number of passes (reserved; per-chunk retry is automatic).
        #[arg(long)]
        passes: Option<u32>,
        /// Add a reverse retry pass.
        #[arg(long = "reverse-pass")]
        reverse_pass: bool,
        /// Sparse output.
        #[arg(long)]
        sparse: bool,
        /// zstd-framed output.
        #[arg(long)]
        zstd: bool,
        /// Hash algorithm: blake3|sha256.
        #[arg(long, default_value = "blake3")]
        hash: String,
        /// Resume from the map.
        #[arg(long)]
        resume: bool,
    },
    /// Verify an image against its map.
    VerifyImage {
        /// Image path.
        image: PathBuf,
        /// Sidecar map path (default IMG.reclaim-map).
        #[arg(long)]
        map: Option<PathBuf>,
    },
    /// Signature catalog tools.
    Sigs {
        #[command(subcommand)]
        cmd: SigsCmd,
    },
    /// List APFS snapshots + reachable checkpoints, or diff one against now.
    Snapshots {
        /// `diskN`, `/dev/rdiskN`, or an image file with an APFS container.
        source: String,
        #[command(subcommand)]
        cmd: Option<SnapshotsCmd>,
    },
    /// Find lost/damaged volumes (FR-SCAN-4); `adopt N` yields a scannable source.
    Volumes {
        /// `diskN`, `/dev/rdiskN`, or an image file (possibly a container).
        source: String,
        #[command(subcommand)]
        cmd: Option<VolumesCmd>,
    },
}

#[derive(Debug, Subcommand)]
enum VolumesCmd {
    /// Adopt proposal N as a `session:<dir>/volume/N` source for `scan`.
    Adopt {
        /// 1-based proposal index from the `volumes` listing.
        n: usize,
    },
}

#[derive(Debug, Subcommand)]
enum SnapshotsCmd {
    /// List files present at a snapshot/checkpoint but absent now.
    Diff {
        /// Snapshot or checkpoint transaction id to diff against.
        #[arg(long)]
        from: u64,
    },
}

#[derive(Debug, Subcommand)]
enum SigsCmd {
    /// List the signature catalog.
    List,
    /// Identify a file against the catalog.
    Test {
        /// File to test.
        file: PathBuf,
    },
}

fn main() -> ExitCode {
    // Doc generation (feature `docgen` only): RECLAIM_GEN_DOCS=<dir> emits man
    // pages, shell completions and docs/cli.md from the clap definition, then
    // exits before any normal parsing/device access.
    #[cfg(feature = "docgen")]
    if let Some(dir) = std::env::var_os("RECLAIM_GEN_DOCS") {
        return docgen::run(std::path::Path::new(&dir));
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return match e.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    ExitCode::from(Exit::Success as u8)
                }
                _ => ExitCode::from(Exit::Usage as u8),
            };
        }
    };

    if cli.prove_readonly {
        reclaim_block::open_log::set_prove_readonly(true);
    }
    if cli.log.is_some() {
        eprintln!("note: --log is reserved; scans write log.ndjson inside the session dir.");
    }

    let result = dispatch(&cli);

    if cli.prove_readonly {
        print_open_log();
    }

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("error: {}", e.message);
            ExitCode::from(e.code as u8)
        }
    }
}

#[allow(clippy::too_many_lines)]
fn dispatch(cli: &Cli) -> Result<Exit, CmdError> {
    let global_session = cli.session.as_deref();
    match &cli.command {
        Command::List => commands::list::run(cli.json, cli.plain),
        Command::Info { source } => commands::info::run(source, cli.json),
        Command::Doctor => commands::doctor::run(cli.json),

        Command::Scan {
            source,
            quick,
            deep,
            families,
            sig_ids,
            range,
            unallocated_only,
            block_size,
            brute_force,
            no_keep_corrupted,
            max_file_size,
            threads: _,
            checkpoint,
            resume,
        } => {
            let block = parse_block_size(block_size)?;
            let range = match range {
                Some(r) => Some(
                    util::parse_range(r)
                        .ok_or_else(|| CmdError::new(Exit::Usage, format!("bad --range {r:?}")))?,
                ),
                None => None,
            };
            let max = util::parse_size(max_file_size)
                .ok_or_else(|| CmdError::new(Exit::Usage, "bad --max-file-size"))?;
            let checkpoint_secs = parse_secs(checkpoint)?;
            let args = commands::scan::ScanArgs {
                source,
                session: global_session,
                quick: *quick,
                deep: *deep,
                families: families.clone(),
                sig_ids: sig_ids.clone(),
                block_size: block,
                brute_force: *brute_force,
                keep_corrupted: !*no_keep_corrupted,
                max_file_size: max,
                range,
                unallocated_only: *unallocated_only,
                checkpoint_secs,
                resume: *resume,
                json: cli.json,
                quiet: cli.quiet,
            };
            commands::scan::run(&args)
        }

        Command::Results {
            session,
            family,
            exts,
            min_size,
            after,
            path,
            min_score,
            engine,
            full_only,
            deleted_only,
            sort,
            limit,
            format,
        } => {
            let min = opt_size(min_size)?;
            let args = commands::results::ResultsArgs {
                session_positional: session.as_deref(),
                session_global: global_session,
                family: family.clone(),
                exts: exts.clone(),
                min_size: min,
                after: after.clone(),
                path_glob: path.clone(),
                min_score: *min_score,
                engine: engine.clone(),
                full_only: *full_only,
                deleted_only: *deleted_only,
                sort: sort.clone(),
                limit: *limit,
                format: format.clone(),
                plain: cli.plain,
            };
            commands::results::run(&args)
        }

        Command::Recover {
            session,
            dest,
            ids,
            all,
            family,
            exts,
            min_size,
            full_only,
            preserve_paths,
            flat,
            collision,
            verify,
            no_fragments: _,
            allow_same_device,
        } => {
            let coll = Collision::parse(collision).ok_or_else(|| {
                CmdError::new(Exit::Usage, format!("bad --collision {collision:?}"))
            })?;
            let ids = match ids {
                Some(spec) => Some(read_ids(spec)?),
                None => None,
            };
            let min = opt_size(min_size)?;
            let filter = QueryFilter {
                family: family.clone(),
                exts: exts.clone(),
                min_size: min,
                full_only: *full_only,
                ..Default::default()
            };
            let args = commands::recover::RecoverArgs {
                session_positional: Some(session.as_path()),
                session_global: global_session,
                dest: dest.clone(),
                ids,
                all: *all,
                filter,
                preserve_paths: *preserve_paths,
                flat: *flat,
                collision: coll,
                verify: *verify,
                allow_same_device: *allow_same_device,
                quiet: cli.quiet,
            };
            commands::recover::run(&args)
        }

        Command::Report {
            session,
            out,
            format,
        } => commands::report::run(
            Some(session.as_path()),
            global_session,
            out,
            format.as_deref(),
        ),

        Command::Preview {
            id,
            session,
            out,
            thumb,
        } => commands::preview::run(
            id,
            session.as_deref(),
            global_session,
            out.as_deref(),
            *thumb,
        ),

        Command::Image {
            source,
            out,
            map,
            block,
            retries,
            skip: _,
            passes: _,
            reverse_pass,
            sparse,
            zstd,
            hash,
            resume,
        } => {
            let block = match block {
                Some(b) => Some(
                    u32::try_from(
                        util::parse_size(b)
                            .ok_or_else(|| CmdError::new(Exit::Usage, "bad --block"))?,
                    )
                    .map_err(|_| CmdError::new(Exit::Usage, "--block too large"))?,
                ),
                None => None,
            };
            let args = commands::image::ImageArgs {
                source,
                out: out.clone(),
                map: map.clone(),
                block,
                retries: *retries,
                reverse_pass: *reverse_pass,
                sparse: *sparse,
                zstd: *zstd,
                hash: hash.clone(),
                resume: *resume,
                quiet: cli.quiet,
            };
            commands::image::run(&args)
        }

        Command::VerifyImage { image, map } => commands::image::verify(image, map.as_deref()),

        Command::Sigs { cmd } => match cmd {
            SigsCmd::List => commands::sigs::list(cli.json, cli.plain),
            SigsCmd::Test { file } => commands::sigs::test(file, cli.json),
        },

        Command::Snapshots { source, cmd } => match cmd {
            None => commands::snapshots::list(source, cli.json),
            Some(SnapshotsCmd::Diff { from }) => commands::snapshots::diff(source, *from, cli.json),
        },

        Command::Volumes { source, cmd } => match cmd {
            None => commands::volumes::run(source, global_session, cli.json),
            Some(VolumesCmd::Adopt { n }) => {
                commands::volumes::adopt(source, *n, global_session, cli.json)
            }
        },
    }
}

fn parse_block_size(s: &str) -> Result<Option<u32>, CmdError> {
    if s == "auto" {
        return Ok(None);
    }
    let v = util::parse_size(s).ok_or_else(|| CmdError::new(Exit::Usage, "bad --block-size"))?;
    let v = u32::try_from(v).map_err(|_| CmdError::new(Exit::Usage, "--block-size too large"))?;
    Ok(Some(v))
}

fn opt_size(s: &Option<String>) -> Result<Option<u64>, CmdError> {
    match s {
        Some(x) => Ok(Some(util::parse_size(x).ok_or_else(|| {
            CmdError::new(Exit::Usage, format!("bad size {x:?}"))
        })?)),
        None => Ok(None),
    }
}

fn parse_secs(s: &str) -> Result<u64, CmdError> {
    let t = s.strip_suffix('s').unwrap_or(s);
    t.parse::<u64>()
        .map_err(|_| CmdError::new(Exit::Usage, format!("bad interval {s:?}")))
}

fn read_ids(spec: &str) -> Result<Vec<String>, CmdError> {
    let text = if spec == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| CmdError::internal(format!("read stdin: {e}")))?;
        s
    } else {
        std::fs::read_to_string(spec)
            .map_err(|e| CmdError::not_found(format!("read {spec}: {e}")))?
    };
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Print the audited open log (for `--prove-readonly`).
fn print_open_log() {
    let log = reclaim_block::open_log::snapshot();
    eprintln!("--- prove-readonly: {} open() call(s) ---", log.len());
    for rec in &log {
        let mode = if rec.write_intent {
            "WRITE-INTENT!"
        } else {
            "O_RDONLY"
        };
        eprintln!(
            "  open({}) flags={:#x} [{mode}]",
            rec.path.display(),
            rec.flags
        );
    }
    if reclaim_block::open_log::any_write_intent() {
        eprintln!("  WARNING: a source was opened with write intent — this is a P0 bug.");
    } else {
        eprintln!("  all opens were read-only.");
    }
}
