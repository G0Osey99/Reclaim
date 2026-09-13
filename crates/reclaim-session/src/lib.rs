//! Reclaim session store & scan orchestration (docs/plan/03 §2.5, build guide
//! Part 3.5).
//!
//! One session is a directory (docs/plan/07 §4): `session.sqlite` (results,
//! progress, events), `source.json` (identity), `plan.json` (engines/options),
//! `log.ndjson` (events), `thumbs/`. Scans run a single sequential read through
//! the carve engine into a single-writer SQLite thread with checkpointing every
//! 5 s / 1 GiB; SIGINT checkpoints and exits (resumable); `--resume` continues
//! from the stored cursor with a fixed block size so the result set is identical
//! (deterministic ids).

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod events;
pub mod id;
pub mod meta;
pub mod recover;
pub mod scan;
pub mod store;
pub mod structs;

pub use events::Event;
pub use meta::MetaReport;
pub use recover::{Collision, RecoverOpts, RecoverSummary, RecoveredFile};
pub use scan::{ScanConfig, ScanReport};
pub use store::{CarvedRecord, EntryRow, ProposalRow, QueryFilter, Sort, Store};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Session errors.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// SQLite failure.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// JSON (de)serialization failure.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Source identity mismatch on reopen (docs/plan/07 §4).
    #[error("source mismatch: {0}")]
    SourceMismatch(String),
    /// Other.
    #[error("{0}")]
    Other(String),
}

/// Persisted source identity (`source.json`, docs/plan/07 §4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceInfo {
    /// Stable source id.
    pub source_id: String,
    /// Size in bytes.
    pub size: u64,
    /// Logical sector size.
    pub sector_size: u32,
    /// blake3 of the first 1 MiB — re-identifies the source on `--resume` even
    /// if the device got a different BSD number after a re-plug.
    #[serde(default)]
    pub first_mib_hash: Option<String>,
    /// blake3 of the last 1 MiB.
    #[serde(default)]
    pub last_mib_hash: Option<String>,
}

/// blake3 of the first and last 1 MiB of a source, for `--resume`
/// re-identification (docs/plan/07 §4). A hash is `None` if that region could
/// not be read cleanly.
#[must_use]
pub fn source_hashes(
    src: &std::sync::Arc<dyn reclaim_block::BlockSource>,
) -> (Option<String>, Option<String>) {
    const MIB: u64 = 1024 * 1024;
    let len = src.len();
    let hash_at = |off: u64, n: u64| -> Option<String> {
        if n == 0 {
            return None;
        }
        let mut buf = vec![0u8; n as usize];
        let r = src.read_at(off, &mut buf);
        if r.any_bad() {
            return None;
        }
        Some(blake3::hash(&buf).to_hex().to_string())
    };
    let first = hash_at(0, MIB.min(len));
    let last = if len > MIB {
        // Align the last-MiB read down to the sector size.
        let ss = u64::from(src.sector_size().max(1));
        let start = (len - MIB) / ss * ss;
        hash_at(start, len - start)
    } else {
        first.clone()
    };
    (first, last)
}

/// Persisted scan plan (`plan.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    /// Engines the planner chose (Phase 1: only `carve`).
    pub engines: Vec<String>,
    /// Block size chosen for the alignment gate.
    pub block_size: u32,
}

/// A recovery session backed by a directory.
#[derive(Debug)]
pub struct Session {
    dir: PathBuf,
    store: Store,
    source: SourceInfo,
}

impl Session {
    /// Create (or reuse) a session directory for `source`.
    pub fn create(dir: &Path, source: SourceInfo) -> Result<Self, SessionError> {
        std::fs::create_dir_all(dir)?;
        std::fs::create_dir_all(dir.join("thumbs"))?;
        let store = Store::open(&dir.join("session.sqlite"))?;
        store.put_source(&source.source_id, source.size, source.sector_size)?;
        let s = Session {
            dir: dir.to_path_buf(),
            store,
            source,
        };
        s.write_source_json()?;
        Ok(s)
    }

    /// Open an existing session, verifying the source identity matches `expect`
    /// unless `force`.
    pub fn open(dir: &Path, expect: &SourceInfo, force: bool) -> Result<Self, SessionError> {
        let store = Store::open(&dir.join("session.sqlite"))?;
        let source = match Self::read_source_json(dir) {
            Ok(s) => s,
            Err(_) => expect.clone(),
        };
        if !force && source.source_id != expect.source_id {
            return Err(SessionError::SourceMismatch(format!(
                "session was created for {}, not {} (use --force-source)",
                source.source_id, expect.source_id
            )));
        }
        // Content re-identification (docs/plan/07 §4): if both the session and
        // the current source carry first/last-MiB hashes, they must match — this
        // catches a device that took a different BSD number but is a different
        // disk, and confirms a re-plugged one is the same.
        if !force {
            if let (Some(a), Some(b)) = (&source.first_mib_hash, &expect.first_mib_hash) {
                if a != b {
                    return Err(SessionError::SourceMismatch(format!(
                        "source content changed (first-MiB hash {a} != {b}); use --force-source"
                    )));
                }
            }
            if let (Some(a), Some(b)) = (&source.last_mib_hash, &expect.last_mib_hash) {
                if a != b {
                    return Err(SessionError::SourceMismatch(format!(
                        "source content changed (last-MiB hash {a} != {b}); use --force-source"
                    )));
                }
            }
        }
        Ok(Session {
            dir: dir.to_path_buf(),
            store,
            source: expect.clone(),
        })
    }

    /// The session directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The thumbnails cache directory.
    #[must_use]
    pub fn thumbs_dir(&self) -> PathBuf {
        self.dir.join("thumbs")
    }

    /// The NDJSON event log path.
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("log.ndjson")
    }

    /// The result store.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Mutable store (for inserts during a scan).
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Run the carved↔named merge (docs/plan/03 §5 step 6): carved results whose
    /// range equals a named entry's extents collapse into the named file.
    /// Returns the number of carved rows merged.
    pub fn merge(&mut self) -> Result<u64, SessionError> {
        self.store.merge_carved_into_entries()
    }

    /// The source identity.
    #[must_use]
    pub fn source(&self) -> &SourceInfo {
        &self.source
    }

    /// Persist the plan.
    pub fn write_plan(&self, plan: &Plan) -> Result<(), SessionError> {
        let text = serde_json::to_string_pretty(plan)?;
        std::fs::write(self.dir.join("plan.json"), text)?;
        Ok(())
    }

    fn write_source_json(&self) -> Result<(), SessionError> {
        let text = serde_json::to_string_pretty(&self.source)?;
        std::fs::write(self.dir.join("source.json"), text)?;
        Ok(())
    }

    fn read_source_json(dir: &Path) -> Result<SourceInfo, SessionError> {
        let text = std::fs::read_to_string(dir.join("source.json"))?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Read a session directory's persisted [`SourceInfo`] without opening the
    /// store (used to reopen the original source behind an adopted volume).
    pub fn read_source_info(dir: &Path) -> Result<SourceInfo, SessionError> {
        Self::read_source_json(dir)
    }
}
