//! `reclaim results` — query/filter a session's results (docs/plan/07 §2).

use crate::commands::common::resolve_session;
use crate::exit::{CmdError, CmdResult, Exit};
use crate::util::format_size;
use comfy_table::{Cell, Table};
use reclaim_session::{CarvedRecord, QueryFilter, Sort, Store};
use std::path::Path;

/// Flags for `results`.
pub struct ResultsArgs<'a> {
    pub session_positional: Option<&'a Path>,
    pub session_global: Option<&'a Path>,
    pub family: Option<String>,
    pub exts: Vec<String>,
    pub min_size: Option<u64>,
    pub after: Option<String>,
    pub path_glob: Option<String>,
    pub min_score: Option<u8>,
    pub engine: Option<String>,
    pub full_only: bool,
    pub sort: Option<String>,
    pub limit: Option<usize>,
    pub format: String,
    pub plain: bool,
}

/// Build a query filter from CLI args.
pub fn filter_from(args: &ResultsArgs) -> Result<QueryFilter, CmdError> {
    let sort = match &args.sort {
        Some(s) => Some(
            Sort::parse(s)
                .ok_or_else(|| CmdError::new(Exit::Usage, format!("bad --sort {s:?}")))?,
        ),
        None => None,
    };
    Ok(QueryFilter {
        family: args.family.clone(),
        exts: args.exts.clone(),
        min_size: args.min_size,
        after: args.after.clone(),
        min_score: args.min_score,
        engine: args.engine.clone(),
        full_only: args.full_only,
        path_glob: args.path_glob.clone(),
        ids: None,
        sort,
        limit: args.limit,
    })
}

/// Run `results`.
pub fn run(args: &ResultsArgs) -> CmdResult {
    let dir = resolve_session(args.session_positional, args.session_global)?;
    let store = Store::open(&dir.join("session.sqlite"))
        .map_err(|e| CmdError::not_found(format!("open session {}: {e}", dir.display())))?;
    let filter = filter_from(args)?;
    let records = store
        .query(&filter)
        .map_err(|e| CmdError::internal(e.to_string()))?;

    match args.format.as_str() {
        "ids" => {
            for r in &records {
                println!("{}", r.id);
            }
        }
        "json" => {
            let rows: Vec<serde_json::Value> = records.iter().map(record_json).collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
            );
        }
        "csv" => {
            println!("id,path,family,format,ext,offset,len,validity,score,date");
            for r in &records {
                println!(
                    "{},{},{},{},{},{},{},{},{},{}",
                    r.id,
                    csv(&r.synth_path()),
                    r.family,
                    r.format,
                    r.ext,
                    r.offset,
                    r.len,
                    r.validity,
                    r.score,
                    r.date.as_deref().unwrap_or("")
                );
            }
        }
        "table" => {
            let mut table = Table::new();
            if args.plain {
                table.load_preset(comfy_table::presets::NOTHING);
            }
            table.set_header(vec![
                "PATH", "FORMAT", "SIZE", "VALIDITY", "SCORE", "OFFSET",
            ]);
            for r in &records {
                table.add_row(vec![
                    Cell::new(r.synth_path()),
                    Cell::new(&r.format),
                    Cell::new(format_size(r.len)),
                    Cell::new(&r.validity),
                    Cell::new(r.score),
                    Cell::new(format!("0x{:x}", r.offset)),
                ]);
            }
            println!("{table}");
            eprintln!("{} result(s).", records.len());
        }
        other => {
            return Err(CmdError::new(
                Exit::Usage,
                format!("unknown --format {other:?} (table|json|csv|ids)"),
            ));
        }
    }
    Ok(Exit::Success)
}

fn record_json(r: &CarvedRecord) -> serde_json::Value {
    serde_json::json!({
        "id": r.id,
        "path": r.synth_path(),
        "family": r.family,
        "format": r.format,
        "ext": r.ext,
        "offset": r.offset,
        "len": r.len,
        "validity": r.validity,
        "score": r.score,
        "date": r.date,
        "model": r.model,
        "block_aligned": r.block_aligned,
    })
}

fn csv(s: &str) -> String {
    if s.contains([',', '"']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
