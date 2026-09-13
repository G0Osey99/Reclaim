//! `snapshots` / `snapshot_diff` — the APFS snapshot browser (doc 08 §2.8,
//! docs/plan/04 §3.1). Read-only: opens the source, finds the APFS container,
//! and walks metadata. No root needed for a mounted-image source.

use crate::ffitypes::{DiffEntry, SnapshotInfo, SnapshotList, SnapshotPointKind};
use crate::resolve;
use crate::RcError;
use reclaim_block::{BlockSource, OffsetView};
use std::sync::Arc;

/// List an APFS container's snapshots and reachable checkpoints.
#[uniffi::export]
pub fn snapshots(source: String) -> Result<SnapshotList, RcError> {
    let apfs = open_apfs(&source)?;
    let points = apfs
        .recovery_points()
        .into_iter()
        .map(|p| SnapshotInfo {
            kind: match p.kind {
                fs_apfs::RecoveryKind::Snapshot => SnapshotPointKind::Snapshot,
                fs_apfs::RecoveryKind::Checkpoint => SnapshotPointKind::Checkpoint,
            },
            xid: p.xid,
            name: p.name,
        })
        .collect();
    Ok(SnapshotList {
        volumes: apfs.volume_names(),
        recoverable_xids: apfs.recoverable_xids(),
        points,
    })
}

/// Files present at snapshot/checkpoint `from_xid` but absent now.
#[uniffi::export]
pub fn snapshot_diff(source: String, from_xid: u64) -> Result<Vec<DiffEntry>, RcError> {
    let apfs = open_apfs(&source)?;
    Ok(apfs
        .diff_from(from_xid)
        .into_iter()
        .map(|e| DiffEntry {
            path: e.path.clone().unwrap_or_else(|| e.name.clone()),
            size: e.size,
            state: e.state.label().to_string(),
        })
        .collect())
}

/// Open the APFS container behind `source` (first partition window that probes
/// as APFS).
fn open_apfs(source: &str) -> Result<fs_apfs::Apfs, RcError> {
    let (_resolved, src, _info) = resolve::open_source(source)?;
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
                .map_err(|e| RcError::internal(format!("open APFS: {e}")));
        }
    }
    Err(RcError::not_found(format!(
        "no APFS container found in {source} (snapshots are an APFS feature)"
    )))
}
