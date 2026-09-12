//! `reclaim preview <RESULT-ID>` — write raw extracted bytes or the embedded
//! JPEG thumbnail to stdout or a file (docs/plan/07 §2). Allow-listed write path.

use crate::commands::common::resolve_session;
use crate::exit::{CmdError, CmdResult, Exit};
use crate::source::Resolved;
use reclaim_carve::reader::{Reader, SourceReader};
use reclaim_session::Store;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_PREVIEW: u64 = 64 * 1024 * 1024;

/// Run `preview`.
pub fn run(
    id: &str,
    session_positional: Option<&Path>,
    session_global: Option<&Path>,
    out: Option<&Path>,
    thumb: bool,
) -> CmdResult {
    let dir = resolve_session(session_positional, session_global)?;
    let store = Store::open(&dir.join("session.sqlite"))
        .map_err(|e| CmdError::not_found(format!("open session {}: {e}", dir.display())))?;
    let rec = store
        .get(id)
        .map_err(|e| CmdError::internal(e.to_string()))?
        .ok_or_else(|| CmdError::not_found(format!("no result with id {id}")))?;

    let source_id = std::fs::read_to_string(dir.join("source.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("source_id")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let resolved = Resolved::from_source_id(&source_id)
        .ok_or_else(|| CmdError::not_found("cannot reconstruct source"))?;
    let src = resolved.open()?;
    let reader = SourceReader::new(Arc::clone(&src));

    // Prefer the embedded thumbnail when requested or available.
    let (off, len) = match (thumb, rec.thumb_offset, rec.thumb_len) {
        (_, Some(o), Some(l)) if thumb || rec.family == "raw" => (o, l.min(MAX_PREVIEW)),
        _ => (rec.offset, rec.len.min(MAX_PREVIEW)),
    };

    let mut off_cur = off;
    let mut remaining = len;
    let mut out_writer: Box<dyn Write> = match out {
        Some(p) => Box::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(p)
                .map_err(|e| CmdError::internal(format!("open {}: {e}", p.display())))?,
        ),
        None => Box::new(std::io::stdout().lock()),
    };
    while remaining > 0 {
        let n = remaining.min(4 * 1024 * 1024) as usize;
        let buf = reader.read(off_cur, n);
        out_writer
            .write_all(&buf)
            .map_err(|e| CmdError::internal(format!("write preview: {e}")))?;
        off_cur += n as u64;
        remaining -= n as u64;
    }
    out_writer.flush().ok();
    if let Some(p) = out {
        let _: PathBuf = p.to_path_buf();
        eprintln!("wrote preview of {id} → {}", p.display());
    }
    Ok(Exit::Success)
}
