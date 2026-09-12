//! `reclaim snapshots` — list an APFS volume's snapshots and reachable
//! checkpoint transactions, and diff a chosen point against the current volume
//! (docs/plan/04 §3.1 step 6, docs/plan/06 §6).
//!
//! This is read-only: it opens the source, finds the APFS container, and walks
//! metadata only.

use crate::commands::common::open_source;
use crate::exit::{CmdError, CmdResult, Exit};
use crate::util::format_size;
use reclaim_block::{BlockSource, OffsetView};
use std::sync::Arc;

/// `reclaim snapshots <SOURCE>` — list snapshots + checkpoint xids.
pub fn list(source: &str, json: bool) -> CmdResult {
    let apfs = open_apfs(source)?;
    let points = apfs.recovery_points();
    let volumes = apfs.volume_names();
    let xids = apfs.recoverable_xids();

    if json {
        let arr: Vec<serde_json::Value> = points
            .iter()
            .map(|p| {
                serde_json::json!({
                    "kind": match p.kind {
                        fs_apfs::RecoveryKind::Snapshot => "snapshot",
                        fs_apfs::RecoveryKind::Checkpoint => "checkpoint",
                    },
                    "xid": p.xid,
                    "name": p.name,
                })
            })
            .collect();
        let doc = serde_json::json!({
            "volumes": volumes,
            "recoverable_checkpoint_xids": xids,
            "points": arr,
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(Exit::Success);
    }

    println!(
        "APFS container: {} volume(s): {}",
        volumes.len(),
        volumes.join(", ")
    );
    println!(
        "recoverable checkpoint xids (newest first): {}",
        xids.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let snaps: Vec<_> = points
        .iter()
        .filter(|p| p.kind == fs_apfs::RecoveryKind::Snapshot)
        .collect();
    if snaps.is_empty() {
        println!("snapshots: none (this container has no APFS snapshots).");
        println!(
            "checkpoint history depth: {} transaction(s) reachable.",
            xids.len()
        );
    } else {
        println!("snapshots:");
        for p in &snaps {
            println!("  xid {:>8}  {}", p.xid, p.name);
        }
    }
    println!("diff a point against now:  reclaim snapshots {source} diff --from <xid>");
    Ok(Exit::Success)
}

/// `reclaim snapshots <SOURCE> diff --from <xid>` — files present at `from` but
/// absent now.
pub fn diff(source: &str, from: u64, json: bool) -> CmdResult {
    let apfs = open_apfs(source)?;
    let entries = apfs.diff_from(from);
    if json {
        let arr: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "path": e.path.clone().unwrap_or_else(|| e.name.clone()),
                    "size": e.size,
                    "state": e.state.label(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "from_xid": from, "files": arr }))
                .unwrap_or_default()
        );
        return Ok(Exit::Success);
    }
    if entries.is_empty() {
        println!("no files present at xid {from} are missing now (or that point is unreachable).");
        return Ok(Exit::Success);
    }
    println!(
        "files present at xid {from} but absent now ({}):",
        entries.len()
    );
    for e in &entries {
        println!(
            "  {}  ({})",
            e.path.clone().unwrap_or_else(|| e.name.clone()),
            format_size(e.size)
        );
    }
    Ok(Exit::Success)
}

/// Open the APFS container behind `source` (finds the APFS partition window).
fn open_apfs(source: &str) -> Result<fs_apfs::Apfs, CmdError> {
    let (_resolved, src, _info) = open_source(source)?;
    // Try each partition window; the first that probes as APFS is the container.
    let map = reclaim_part::scan(&src);
    for (start, len) in map.volume_windows(src.len()) {
        if len == 0 {
            continue;
        }
        let view: Arc<dyn BlockSource> = match OffsetView::new(Arc::clone(&src), start, len) {
            Ok(v) => Arc::new(v),
            Err(_) => continue,
        };
        if let Some(probe) = fs_apfs::probe(&view) {
            return fs_apfs::open(view, probe)
                .map_err(|e| CmdError::internal(format!("open APFS: {e}")));
        }
    }
    Err(CmdError::not_found(format!(
        "no APFS container found in {source} (snapshots are an APFS feature)"
    )))
}
