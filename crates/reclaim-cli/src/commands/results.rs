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
    pub deleted_only: bool,
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
        deleted_only: args.deleted_only,
        include_merged: false,
        path_glob: args.path_glob.clone(),
        ids: None,
        sort,
        limit: args.limit,
        offset: None,
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
                "PATH", "FORMAT", "SIZE", "STATE", "VALIDITY", "SCORE", "OFFSET",
            ]);
            for r in &records {
                table.add_row(vec![
                    Cell::new(r.synth_path()),
                    Cell::new(&r.format),
                    Cell::new(format_size(r.len)),
                    Cell::new(r.state.as_deref().unwrap_or(&r.engine)),
                    Cell::new(&r.validity),
                    Cell::new(r.score),
                    Cell::new(format!("0x{:x}", r.offset)),
                ]);
            }
            println!("{table}");
            eprintln!("{} result(s).", records.len());
        }
        "tree" => {
            print_tree(&records);
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
        "engine": r.engine,
        "state": r.state,
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
    // Neutralize spreadsheet formula injection, then quote per RFC 4180.
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// Print recovered results as an indented directory tree (docs/plan/07 §2;
/// build guide Phase-2 prompt D `results --format tree`). Files sort under their
/// synthesized/real paths; directories are derived from the path components.
fn print_tree(records: &[CarvedRecord]) {
    use std::collections::BTreeMap;

    // Build a nested map from path components.
    #[derive(Default)]
    struct Node {
        dirs: BTreeMap<String, Node>,
        files: Vec<(String, u64, String, String)>, // (name, len, state/validity, score-note)
    }

    let mut root = Node::default();
    for r in records {
        let path = r.synth_path();
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        if comps.is_empty() {
            continue;
        }
        let mut node = &mut root;
        for dir in &comps[..comps.len() - 1] {
            node = node.dirs.entry((*dir).to_string()).or_default();
        }
        let fname = comps.last().copied().unwrap_or("?").to_string();
        let tag = r.state.clone().unwrap_or_else(|| r.validity.clone());
        node.files
            .push((fname, r.len, tag, format!("score {}", r.score)));
    }

    fn walk(node: &Node, depth: usize) {
        let indent = "  ".repeat(depth);
        for (name, child) in &node.dirs {
            println!("{indent}{name}/");
            walk(child, depth + 1);
        }
        for (name, len, tag, note) in &node.files {
            println!("{indent}{name}  ({}, {tag}, {note})", format_size(*len));
        }
    }
    walk(&root, 0);
}

#[cfg(test)]
mod tests {
    use super::csv;

    #[test]
    fn csv_neutralizes_formulas_and_quotes() {
        assert_eq!(csv("=cmd|' /C calc'!A0"), "'=cmd|' /C calc'!A0");
        assert_eq!(csv("\tx"), "'\tx");
        assert_eq!(csv("a,b"), "\"a,b\"");
        assert_eq!(csv("q\"q"), "\"q\"\"q\"");
        assert_eq!(csv("plain"), "plain");
    }
}
