//! `reclaim report` — write a JSON/HTML/CSV report of a session (doc 07 §2).
//! Allow-listed write path (writes to the user-chosen OUT).

use crate::commands::common::resolve_session;
use crate::exit::{CmdError, CmdResult, Exit};
use reclaim_report::{render, Format, Summary};
use reclaim_session::{QueryFilter, Store};
use std::path::Path;

/// Run `report`.
pub fn run(
    session_positional: Option<&Path>,
    session_global: Option<&Path>,
    out: &Path,
    format: Option<&str>,
) -> CmdResult {
    let dir = resolve_session(session_positional, session_global)?;
    let store = Store::open(&dir.join("session.sqlite"))
        .map_err(|e| CmdError::not_found(format!("open session {}: {e}", dir.display())))?;

    let fmt = match format {
        Some(f) => Format::parse(f)
            .ok_or_else(|| CmdError::new(Exit::Usage, format!("bad --format {f:?}")))?,
        None => out
            .extension()
            .and_then(|e| Format::parse(&e.to_string_lossy()))
            .ok_or_else(|| {
                CmdError::new(
                    Exit::Usage,
                    "cannot infer report format from extension; pass --format json|html|csv",
                )
            })?,
    };

    let records = store
        .query(&QueryFilter::default())
        .map_err(|e| CmdError::internal(e.to_string()))?;
    let source_id = std::fs::read_to_string(dir.join("source.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("source_id")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());

    let summary = Summary::from_records(&source_id, &records, now_iso());
    let doc = render(&records, &summary, fmt);
    std::fs::write(out, doc)
        .map_err(|e| CmdError::internal(format!("write {}: {e}", out.display())))?;
    eprintln!("wrote {} ({} results).", out.display(), records.len());
    Ok(Exit::Success)
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}
