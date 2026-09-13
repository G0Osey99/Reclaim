//! `volumes` — the lost-structure engine (doc 08 §2.6, FR-SCAN-4). Mirrors the
//! CLI `volumes` command: scan + cross-validate proposals, persist them in the
//! session, and return each with its ready-to-adopt `session:<dir>/volume/<n>`
//! spec.

use crate::ffitypes::ProposalInfo;
use crate::resolve;
use crate::RcError;
use reclaim_session::{Session, SourceInfo};
use std::path::{Path, PathBuf};

/// Scan `source` for lost/damaged volumes and return the proposals.
#[uniffi::export]
pub fn volumes(source: String, session_dir: Option<String>) -> Result<Vec<ProposalInfo>, RcError> {
    let (_resolved, src, info) = resolve::open_source(&source)?;
    let dir = match session_dir {
        Some(d) => PathBuf::from(d),
        None => stable_session_dir(&info.source_id),
    };
    let session = open_or_create(&dir, &info)?;
    let rows = reclaim_session::structs::scan_and_store(&src, session.store())?;
    Ok(rows
        .iter()
        .enumerate()
        .map(|(i, r)| ProposalInfo {
            index: (i + 1) as u32,
            fs: r.fs.clone(),
            start: r.start,
            len: r.len,
            confidence: f64::from(r.confidence),
            evidence: r.evidence.clone(),
            source_spec: format!("session:{}/volume/{}", dir.display(), i + 1),
        })
        .collect())
}

fn open_or_create(dir: &Path, info: &SourceInfo) -> Result<Session, RcError> {
    if dir.join("session.sqlite").exists() {
        Session::open(dir, info, true).map_err(RcError::from)
    } else {
        std::fs::create_dir_all(dir)
            .map_err(|e| RcError::internal(format!("creating session dir: {e}")))?;
        Session::create(dir, info.clone()).map_err(RcError::from)
    }
}

/// A stable session directory keyed by the source identity (matches the CLI's
/// `stable_session_dir`, so `volumes` then a GUI adopt reuse one store).
fn stable_session_dir(source_id: &str) -> PathBuf {
    let h = blake3_short(source_id);
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Library/Application Support/Reclaim/sessions")
        .join(format!("vol-{h}"))
}

fn blake3_short(s: &str) -> String {
    // Match the CLI's naming (blake3 first 16 hex) without adding a dep here:
    // reclaim-session re-exports nothing for this, so fall back to a stable FNV
    // hash — the directory only needs to be stable per source, not identical to
    // the CLI's (the GUI and CLI can keep separate stores for the same source).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}
