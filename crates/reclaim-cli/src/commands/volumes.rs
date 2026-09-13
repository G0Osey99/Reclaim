//! `reclaim volumes <SOURCE> [adopt N]` — the lost-structure engine
//! (docs/plan/07 §2, FR-SCAN-4). Lists proposed recoverable volumes found by
//! [`reclaim_session::structs`], and `adopt` prints the ready-to-scan source
//! spec `session:<dir>/volume/<n>`.

use crate::commands::common::{open_source, stable_session_dir};
use crate::exit::{CmdError, CmdResult, Exit};
use crate::util::format_size;
use reclaim_session::{ProposalRow, Session, SourceInfo};
use std::path::{Path, PathBuf};

/// Resolve the session dir for the volumes flow: `--session` if given, else a
/// stable directory keyed by the source identity (so a later `adopt` reuses it).
fn session_dir(global: Option<&Path>, info: &SourceInfo) -> PathBuf {
    global
        .map(Path::to_path_buf)
        .unwrap_or_else(|| stable_session_dir(&info.source_id))
}

/// Open or create the session at `dir` for `info`.
fn open_or_create(dir: &Path, info: &SourceInfo) -> Result<Session, CmdError> {
    if dir.join("session.sqlite").exists() {
        Session::open(dir, info, true)
            .map_err(|e| CmdError::internal(format!("opening session {}: {e}", dir.display())))
    } else {
        std::fs::create_dir_all(dir)
            .map_err(|e| CmdError::internal(format!("creating session dir: {e}")))?;
        Session::create(dir, info.clone())
            .map_err(|e| CmdError::internal(format!("creating session: {e}")))
    }
}

/// `reclaim volumes <SOURCE>` — scan + list proposals.
pub fn run(source: &str, global_session: Option<&Path>, json: bool) -> CmdResult {
    let (_resolved, src, info) = open_source(source)?;
    let dir = session_dir(global_session, &info);
    let session = open_or_create(&dir, &info)?;
    let rows = reclaim_session::structs::scan_and_store(&src, session.store())
        .map_err(|e| CmdError::internal(format!("structure scan: {e}")))?;
    print_proposals(&rows, &dir, json)?;
    Ok(Exit::Success)
}

/// `reclaim volumes <SOURCE> adopt N` — confirm proposal N is adoptable and
/// print its source spec.
pub fn adopt(source: &str, n: usize, global_session: Option<&Path>, json: bool) -> CmdResult {
    let (_resolved, src, info) = open_source(source)?;
    let dir = session_dir(global_session, &info);
    let session = open_or_create(&dir, &info)?;
    // Ensure proposals are present (scan if this is a fresh session).
    let mut rows = session
        .store()
        .list_proposals()
        .map_err(|e| CmdError::internal(format!("reading proposals: {e}")))?;
    if rows.is_empty() {
        rows = reclaim_session::structs::scan_and_store(&src, session.store())
            .map_err(|e| CmdError::internal(format!("structure scan: {e}")))?;
    }
    if n == 0 || n > rows.len() {
        return Err(CmdError::new(
            Exit::Usage,
            format!("no proposal {n} (found {})", rows.len()),
        ));
    }
    let row = rows
        .get(n - 1)
        .ok_or_else(|| CmdError::not_found("proposal"))?;
    if row.len == 0 {
        return Err(CmdError::new(
            Exit::Usage,
            format!("proposal {n} ({}) is not independently adoptable", row.fs),
        ));
    }
    let spec = format!("session:{}/volume/{n}", dir.display());
    // Confirm the spec actually resolves + opens.
    let resolved = crate::source::Resolved::parse(&spec)?;
    let _ = resolved.open()?;
    if json {
        let v = serde_json::json!({
            "adopted": n, "source": spec, "fs": row.fs,
            "start": row.start, "len": row.len,
        });
        println!("{v}");
    } else {
        println!(
            "adopted volume {n}: {} @ 0x{:x} ({})",
            row.fs,
            row.start,
            format_size(row.len)
        );
        println!("scan it with:  reclaim scan '{spec}'");
    }
    Ok(Exit::Success)
}

fn print_proposals(rows: &[ProposalRow], dir: &Path, json: bool) -> Result<(), CmdError> {
    if json {
        let arr: Vec<_> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                serde_json::json!({
                    "n": i + 1, "fs": r.fs, "start": r.start, "len": r.len,
                    "confidence": r.confidence, "evidence": r.evidence,
                    "source": format!("session:{}/volume/{}", dir.display(), i + 1),
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(arr));
        return Ok(());
    }
    if rows.is_empty() {
        println!("no lost volumes proposed.");
        return Ok(());
    }
    println!("proposed volumes (session {}):", dir.display());
    for (i, r) in rows.iter().enumerate() {
        let len = if r.len == 0 {
            "?".to_string()
        } else {
            format_size(r.len)
        };
        println!(
            "  {:>2}  {:<10} @ 0x{:<12x} {:>10}  conf {:.2}",
            i + 1,
            r.fs,
            r.start,
            len,
            r.confidence
        );
        println!("      {}", r.evidence);
    }
    println!("\nadopt one with:  reclaim volumes <SOURCE> adopt <N>");
    Ok(())
}
