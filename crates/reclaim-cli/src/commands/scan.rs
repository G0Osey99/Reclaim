//! `reclaim scan` — metadata (quick) and/or carve (deep) a source into a
//! session, then merge carved results into named filesystem entries
//! (docs/plan/07 §2; build guide Phase-2 prompt D).

use crate::commands::common::{open_source, resolve_session};
use crate::exit::{CmdError, CmdResult, Exit};
use crate::util::format_size;
use reclaim_carve::engine::CarveOptions;
use reclaim_session::{ScanConfig, Session};
use std::path::Path;

/// Flags for `scan`.
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

    // Default (no pass flag) runs quick then deep; a flag selects just that pass.
    let run_quick = args.quick || !args.deep;
    let run_deep = args.deep || !args.quick;

    let dir = resolve_session(None, args.session)?;
    let mut session = if args.resume && dir.join("session.sqlite").exists() {
        Session::open(&dir, &info, false).map_err(|e| CmdError::internal(e.to_string()))?
    } else {
        Session::create(&dir, info.clone()).map_err(|e| CmdError::internal(e.to_string()))?
    };
    if !args.quiet {
        eprintln!("scanning {} → session {}", args.source, dir.display());
    }

    let quiet = args.quiet;
    let json = args.json;

    // --- Quick (metadata) pass ---
    let mut bitmaps = Vec::new();
    if run_quick {
        let meta = session
            .run_quick(&src, &mut |ev| emit_quick(ev, json, quiet))
            .map_err(|e| CmdError::internal(e.to_string()))?;
        bitmaps = meta.bitmaps;
        if !quiet {
            eprintln!(
                "quick: scheme {}, {} volume(s), {} named entr{} ({} deleted/orphaned).",
                meta.scheme,
                meta.volumes_handled,
                meta.entries,
                if meta.entries == 1 { "y" } else { "ies" },
                meta.deleted
            );
        }
    }

    // --- Deep (carve) pass ---
    if run_deep {
        let carve = CarveOptions {
            brute_force: args.brute_force,
            block_size: args.block_size.unwrap_or(0),
            families: args.families.clone(),
            sig_ids: args.sig_ids.clone(),
            keep_corrupted: args.keep_corrupted,
            max_file_size: args.max_file_size,
            range: args.range,
        };
        let cfg = ScanConfig {
            carve,
            resume: args.resume,
            checkpoint_secs: args.checkpoint_secs,
            emit_events: json,
            cancel: None,
        };
        let report = session
            .run_carve(&src, &cfg, &mut |ev| {
                if json {
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

        // Label carved results allocated/unallocated and, with --unallocated-only,
        // hide the allocated ones (a live file's content is not a deletion).
        if !bitmaps.is_empty() {
            let (alloc, unalloc) = session
                .label_carved(&bitmaps, args.unallocated_only)
                .map_err(|e| CmdError::internal(e.to_string()))?;
            if !quiet {
                eprintln!(
                    "carve labels: {unalloc} unallocated, {alloc} allocated{}",
                    if args.unallocated_only {
                        " (allocated hidden)"
                    } else {
                        ""
                    }
                );
            }
        } else if args.unallocated_only && !quiet {
            eprintln!("note: --unallocated-only needs the FS bitmap; run without --deep-only, or on a supported volume.");
        }

        if report.interrupted {
            eprintln!(
                "interrupted — resume with `reclaim scan {} --session {} --resume`.",
                args.source,
                dir.display()
            );
            return Ok(Exit::Interrupted);
        }
    }

    // --- Merge carved ↔ named (only meaningful when both passes ran) ---
    if run_quick && run_deep {
        let merged = session
            .merge()
            .map_err(|e| CmdError::internal(e.to_string()))?;
        if !quiet && merged > 0 {
            eprintln!("merge: {merged} carved result(s) collapsed into named entries.");
        }
    }

    let total = session
        .store()
        .count()
        .map_err(|e| CmdError::internal(e.to_string()))?;
    if !quiet {
        eprintln!(
            "{total} total result row(s) in session (use `reclaim results {}`).",
            dir.display()
        );
    }
    Ok(Exit::Success)
}

fn emit_quick(ev: &reclaim_session::Event, json: bool, quiet: bool) {
    if json {
        println!("{}", ev.to_ndjson());
    } else if !quiet {
        if let reclaim_session::Event::Warning { msg } = ev {
            eprintln!("  {msg}");
        }
    }
}
