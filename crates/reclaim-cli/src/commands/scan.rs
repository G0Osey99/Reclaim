//! `reclaim scan` — deep-carve a source into a session (docs/plan/07 §2).

use crate::commands::common::{open_source, resolve_session};
use crate::exit::{CmdError, CmdResult, Exit};
use crate::util::format_size;
use reclaim_carve::engine::CarveOptions;
use reclaim_session::{ScanConfig, Session};
use std::path::Path;

/// Flags for `scan` (subset of doc 07 §2 that applies in Phase 1).
pub struct ScanArgs<'a> {
    pub source: &'a str,
    pub session: Option<&'a Path>,
    pub quick: bool,
    pub deep: bool,
    pub families: Vec<String>,
    pub sig_ids: Vec<String>,
    pub block_size: Option<u32>,
    pub brute_force: bool,
    pub keep_corrupted: bool,
    pub max_file_size: u64,
    pub range: Option<(u64, u64)>,
    pub unallocated_only: bool,
    pub checkpoint_secs: u64,
    pub resume: bool,
    pub json: bool,
    pub quiet: bool,
}

/// Run `scan`.
pub fn run(args: &ScanArgs) -> CmdResult {
    let (_resolved, src, info) = open_source(args.source)?;

    // Phase 1 has only the carve engine. `--quick` alone (metadata) has nothing
    // to run yet (Phase 2).
    if args.quick && !args.deep {
        eprintln!(
            "note: metadata engines arrive in Phase 2; only --deep carving is available now."
        );
        return Ok(Exit::Success);
    }

    let dir = resolve_session(None, args.session)?;
    let mut session = if args.resume && dir.join("session.sqlite").exists() {
        Session::open(&dir, &info, false).map_err(|e| CmdError::internal(e.to_string()))?
    } else {
        Session::create(&dir, info.clone()).map_err(|e| CmdError::internal(e.to_string()))?
    };
    if !args.quiet {
        eprintln!("scanning {} → session {}", args.source, dir.display());
    }

    let carve = CarveOptions {
        brute_force: args.brute_force,
        block_size: args.block_size.unwrap_or(0),
        families: args.families.clone(),
        sig_ids: args.sig_ids.clone(),
        keep_corrupted: args.keep_corrupted,
        max_file_size: args.max_file_size,
        range: args.range,
    };
    let _ = args.unallocated_only; // no FS bitmap until Phase 2 (whole = unallocated)
    let cfg = ScanConfig {
        carve,
        resume: args.resume,
        checkpoint_secs: args.checkpoint_secs,
        emit_events: args.json,
        cancel: None,
    };

    let quiet = args.quiet;
    let json = args.json;
    let report = session
        .run_carve(&src, &cfg, &mut |ev| {
            if json {
                // NDJSON events on stdout (human text stays on stderr).
                println!("{}", ev.to_ndjson());
            } else if !quiet {
                if let reclaim_session::Event::Progress { pct, rate, .. } = ev {
                    eprint!("\rcarve  {pct:5.1}%  {}/s        ", format_size(*rate));
                }
            }
        })
        .map_err(|e| CmdError::internal(e.to_string()))?;

    if !json && !quiet {
        eprintln!();
    }
    let total = session
        .store()
        .count()
        .map_err(|e| CmdError::internal(e.to_string()))?;
    if !quiet {
        eprintln!(
            "found {} result(s) this run; {} total in session ({:.1}s).",
            report.found, total, report.elapsed
        );
    }

    if report.interrupted {
        eprintln!(
            "interrupted — resume with `reclaim scan {} --session {} --resume`.",
            args.source,
            dir.display()
        );
        return Ok(Exit::Interrupted);
    }
    Ok(Exit::Success)
}
