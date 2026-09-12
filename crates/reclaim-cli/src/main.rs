//! `reclaim` — read-only disk / file / photo recovery CLI.
//!
//! Phase 0 ships the read-only groundwork: `list`, `info` and `doctor`. Every
//! device open is `O_RDONLY`; `--prove-readonly` prints the audited open log so
//! an auditor can confirm no source was ever opened writable (docs/plan/07 §5.3).

mod commands;
mod exit;
mod source;
mod util;

use clap::{Parser, Subcommand};
use exit::{CmdError, Exit};
use std::path::PathBuf;
use std::process::ExitCode;

/// Read-only disk, file and photo recovery.
#[derive(Debug, Parser)]
#[command(
    name = "reclaim",
    version,
    about = "Read-only disk/file/photo recovery (Phase 0: list · info · doctor)",
    disable_help_subcommand = true
)]
struct Cli {
    /// Emit machine-readable JSON on stdout (human text goes to stderr).
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

    /// Session file/dir (reserved; sessions land in Phase 1).
    #[arg(long, global = true, value_name = "PATH")]
    session: Option<PathBuf>,

    /// Structured log file (reserved; honored from Phase 1 — logs to stderr now).
    #[arg(long, global = true, value_name = "FILE")]
    log: Option<PathBuf>,

    /// Log every device open() with its flags to stderr (read-only audit).
    #[arg(long = "prove-readonly", global = true)]
    prove_readonly: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Enumerate sources (devices, partitions, volumes).
    List,
    /// Show details for a source: geometry, mounts, health, read-speed probe.
    Info {
        /// Source: `diskN`, `/dev/rdiskN`, or an image file path.
        source: String,
    },
    /// Permissions / root / Full Disk Access / SIP self-check.
    Doctor,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // Help/version print to stdout with success; real usage errors to
            // stderr with exit 1 (docs/plan/07 §3).
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
        eprintln!("note: --log is reserved; Phase 0 logs to stderr only.");
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

fn dispatch(cli: &Cli) -> Result<Exit, CmdError> {
    match &cli.command {
        Command::List => commands::list::run(cli.json, cli.plain),
        Command::Info { source } => commands::info::run(source, cli.json),
        Command::Doctor => commands::doctor::run(cli.json),
    }
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
