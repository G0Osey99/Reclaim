//! The `Session` object and the scan pipeline (doc 08 §5). `start_scan` opens
//! the source, spawns the same quick → deep → structure → merge pipeline the CLI
//! runs (from `reclaim-session`), and returns a handle immediately; events are
//! delivered to the [`crate::EventSink`] from the scan thread. Browsing methods
//! (`query`, `count`, `get`, `preview`, `recover`, `report`) open their own read
//! connection on the session's SQLite (WAL), so the GUI can page results and
//! preview thumbnails *while the scan is still writing* (doc 08 §4).

use crate::ffitypes::*;
use crate::resolve::{self, Resolved};
use crate::{EventSink, RcError};
use reclaim_carve::engine::CarveOptions;
use reclaim_carve::reader::{Reader, SourceReader};
use reclaim_session::{ScanConfig, SourceInfo, Store};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const MAX_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;
const PREVIEW_TEXT_BYTES: usize = 128 * 1024;

/// A recovery session: a directory holding `session.sqlite`, `source.json`,
/// events and thumbnails. Created by [`start_scan`] (with a running scan) or
/// [`open_session`] (browse an existing one, incl. a CLI-produced session).
#[derive(uniffi::Object)]
pub struct Session {
    dir: PathBuf,
    /// The source spec to reopen the device/image for preview/recover; falls
    /// back to `source.json`'s `source_id` when `None` (opened, not scanned).
    source_spec: Mutex<Option<String>>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
    scan: Mutex<Option<JoinHandle<Result<ScanSummary, RcError>>>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("dir", &self.dir).finish()
    }
}

/// Open an existing session directory for browsing (results, previews, recover,
/// reports, snapshots) — including one produced by the `reclaim` CLI.
#[uniffi::export]
pub fn open_session(dir: String) -> Result<Arc<Session>, RcError> {
    let path = PathBuf::from(&dir);
    if !path.join("session.sqlite").exists() {
        return Err(RcError::not_found(format!("no session at {dir}")));
    }
    Ok(Arc::new(Session {
        dir: path,
        source_spec: Mutex::new(None),
        cancel: Mutex::new(None),
        scan: Mutex::new(None),
    }))
}

/// Start (or resume) a scan of `source` into `session_dir` (default: a temp dir
/// under the session store). Returns immediately; the scan runs on a background
/// thread and emits events to `sink` (doc 08 §5).
#[uniffi::export]
pub fn start_scan(
    source: String,
    session_dir: Option<String>,
    opts: ScanOptions,
    sink: Box<dyn EventSink>,
) -> Result<Arc<Session>, RcError> {
    let (_resolved, src, info) = resolve::open_source(&source)?;
    let dir = match session_dir {
        Some(d) => PathBuf::from(d),
        None => default_session_dir(&info.source_id),
    };
    let handle = Arc::new(Session {
        dir: dir.clone(),
        source_spec: Mutex::new(Some(source)),
        cancel: Mutex::new(None),
        scan: Mutex::new(None),
    });
    handle.spawn_scan(src, info, opts, sink)?;
    Ok(handle)
}

#[uniffi::export]
impl Session {
    /// The session directory.
    #[must_use]
    pub fn dir(&self) -> String {
        self.dir.to_string_lossy().to_string()
    }

    /// The persisted source id (`source.json`), if present.
    #[must_use]
    pub fn source_id(&self) -> Option<String> {
        read_source_id(&self.dir)
    }

    /// Total result rows in the session.
    pub fn count(&self) -> Result<u64, RcError> {
        Ok(self.open_store()?.count()?)
    }

    /// Count results matching a filter (page total / rail counts).
    pub fn count_matching(&self, filter: ResultFilter) -> Result<u64, RcError> {
        Ok(self
            .open_store()?
            .count_matching(&filter.to_count_query())?)
    }

    /// `(family, count)` for the filter rail (doc 08 §2.3).
    pub fn family_counts(&self, deleted_only: bool) -> Result<Vec<FamilyCount>, RcError> {
        let counts = self.open_store()?.family_counts(deleted_only, false)?;
        Ok(counts
            .into_iter()
            .map(|(family, count)| FamilyCount { family, count })
            .collect())
    }

    /// A page of results (paged from SQLite by `offset`/`limit`, doc 08 §4).
    pub fn query(
        &self,
        filter: ResultFilter,
        offset: u32,
        limit: u32,
    ) -> Result<ResultPage, RcError> {
        let store = self.open_store()?;
        let total = store.count_matching(&filter.to_count_query())?;
        let rows = store.query(&filter.to_query(offset, limit))?;
        Ok(ResultPage {
            total,
            offset,
            items: rows.iter().map(ResultRecord::from).collect(),
        })
    }

    /// One result by id.
    pub fn get(&self, id: String) -> Result<Option<ResultRecord>, RcError> {
        Ok(self.open_store()?.get(&id)?.map(|r| ResultRecord::from(&r)))
    }

    /// Render a preview of a result without touching the source beyond its own
    /// extents (doc 08 §2.3, FR-RES-2). Raster images and embedded JPEGs decode
    /// in-process; text/hex are byte transforms.
    pub fn preview(
        &self,
        id: String,
        kind: PreviewKind,
        max_px: u32,
    ) -> Result<PreviewResult, RcError> {
        let store = self.open_store()?;
        let rec = store
            .get(&id)?
            .ok_or_else(|| RcError::not_found(format!("no result {id}")))?;
        let reader = self.open_reader()?;
        match kind {
            PreviewKind::Text => {
                let bytes = read_bytes(&reader, &rec.extents(), PREVIEW_TEXT_BYTES as u64);
                let text = reclaim_preview::text_preview(&bytes, PREVIEW_TEXT_BYTES);
                Ok(PreviewResult {
                    mime: "text/plain".to_string(),
                    bytes: text.into_bytes(),
                })
            }
            PreviewKind::Hex => {
                let bytes = read_bytes(&reader, &rec.extents(), 8 * 1024);
                let text = reclaim_preview::hex_preview(&bytes, 8 * 1024);
                Ok(PreviewResult {
                    mime: "text/plain".to_string(),
                    bytes: text.into_bytes(),
                })
            }
            PreviewKind::Thumbnail | PreviewKind::Image => {
                // Prefer the embedded JPEG for formats we cannot decode fully in
                // Rust (HEIC/RAW); otherwise decode the file's own bytes.
                let use_thumb = rec.thumb_offset.is_some()
                    && rec.thumb_len.is_some()
                    && !reclaim_preview::can_decode_ext(&rec.ext);
                let bytes = if use_thumb {
                    let (o, l) = (
                        rec.thumb_offset.unwrap_or(0),
                        rec.thumb_len.unwrap_or(0).min(MAX_PREVIEW_BYTES),
                    );
                    read_bytes(&reader, &[(o, l)], MAX_PREVIEW_BYTES)
                } else {
                    read_bytes(&reader, &rec.extents(), MAX_PREVIEW_BYTES)
                };
                let png = match kind {
                    PreviewKind::Thumbnail => reclaim_preview::image_thumbnail(&bytes, max_px),
                    _ => reclaim_preview::image_full_png(&bytes),
                }
                .map_err(|e| RcError::internal(format!("preview: {e}")))?;
                Ok(PreviewResult {
                    mime: "image/png".to_string(),
                    bytes: png,
                })
            }
        }
    }

    /// Extract one result's bytes to a temp file in the session dir so the Swift
    /// side can hand it to PDFKit / AVFoundation (doc 08 §2.3). Returns the path.
    pub fn extract_temp(&self, id: String) -> Result<String, RcError> {
        let store = self.open_store()?;
        let rec = store
            .get(&id)?
            .ok_or_else(|| RcError::not_found(format!("no result {id}")))?;
        let reader = self.open_reader()?;
        let path =
            reclaim_session::recover::extract_to_temp(&self.dir, &rec, &reader, MAX_PREVIEW_BYTES)?;
        Ok(path.to_string_lossy().to_string())
    }

    /// Validate a recover destination (same-disk refusal + free-space warning,
    /// doc 08 §2.4, docs/plan/06 §10).
    pub fn check_destination(
        &self,
        dest: String,
        needed_bytes: u64,
    ) -> Result<DestinationCheck, RcError> {
        let resolved = self.resolve_source()?;
        let dest_path = Path::new(&dest);
        let same_disk = resolve::dest_on_same_disk(&resolved, dest_path);
        let free = resolve::free_space(dest_path);
        let low_space = free.is_some_and(|f| (f as f64) < (needed_bytes as f64) * 1.1);
        let reason = if same_disk {
            Some("destination is on the same physical disk as the source (data-loss risk)".into())
        } else {
            None
        };
        Ok(DestinationCheck {
            ok: !same_disk,
            same_disk,
            free_bytes: free,
            low_space,
            reason,
        })
    }

    /// Recover the selected results to `opts.dest` (doc 08 §2.4). Refuses a
    /// same-disk destination unless `allow_same_device`; verifies after copy when
    /// requested; writes a manifest; streams per-file progress to `sink`.
    pub fn recover(
        &self,
        ids: Vec<String>,
        opts: RecoverOptions,
        sink: Box<dyn EventSink>,
    ) -> Result<RecoverOutcome, RcError> {
        if ids.is_empty() {
            return Err(RcError::usage("no results selected for recovery"));
        }
        let resolved = self.resolve_source()?;
        let dest = PathBuf::from(&opts.dest);
        if !opts.allow_same_device && resolve::dest_on_same_disk(&resolved, &dest) {
            return Err(RcError::refused(format!(
                "destination {} is on the same disk as the source — refusing (data-loss risk).",
                dest.display()
            )));
        }
        let store = self.open_store()?;
        let filter = reclaim_session::QueryFilter {
            ids: Some(ids.clone()),
            include_merged: true,
            ..Default::default()
        };
        let records = store.query(&filter)?;
        if records.is_empty() {
            return Err(RcError::not_found(
                "none of the selected ids are in the session",
            ));
        }
        let reader = self.open_reader_for(&resolved)?;
        let rec_opts = reclaim_session::RecoverOpts {
            preserve_paths: opts.preserve_paths,
            flat: opts.flat,
            collision: opts.collision.into(),
            verify: opts.verify,
        };
        let source_id = read_source_id(&self.dir).unwrap_or_default();
        let mut on_file = |done: u64, total: u64, f: &reclaim_session::RecoveredFile| {
            sink.on_event(RcEvent::RecoverProgress {
                done,
                total,
                path: f.path.clone(),
            });
        };
        let summary = reclaim_session::recover::recover_records(
            &dest,
            &source_id,
            &records,
            &reader,
            &rec_opts,
            &mut on_file,
        )?;
        Ok(RecoverOutcome {
            recovered: summary.recovered,
            skipped: summary.skipped,
            partial: summary.partial,
            verify_failed: summary.verify_failed,
            bytes: summary.bytes,
            manifest_path: summary.manifest_path,
            dest: dest.to_string_lossy().to_string(),
            files: summary
                .files
                .iter()
                .map(|f| RecoveredEntry {
                    id: f.id.clone(),
                    path: f.path.clone(),
                    len: f.len,
                    validity: f.validity.clone(),
                    blake3: f.blake3.clone(),
                    verified: f.verified,
                })
                .collect(),
        })
    }

    /// Render a report over all results (doc 08 §2.9, FR-RES-5).
    pub fn report(&self, format: ReportFormat) -> Result<String, RcError> {
        let store = self.open_store()?;
        let records = store.query(&reclaim_session::QueryFilter::default())?;
        let source_id = read_source_id(&self.dir).unwrap_or_default();
        let generated = now_string();
        let summary = reclaim_report::Summary::from_records(&source_id, &records, generated);
        Ok(reclaim_report::render(&records, &summary, format.into()))
    }

    /// True while a scan thread is running.
    #[must_use]
    pub fn is_scanning(&self) -> bool {
        self.scan
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|h| !h.is_finished()))
            .unwrap_or(false)
    }

    /// Request a pause: the carve pass checkpoints and stops; the session stays
    /// resumable (doc 08 §3, "auto-pause"). Resume with [`Session::resume_scan`].
    pub fn pause(&self) {
        self.signal_cancel();
    }

    /// Stop the scan permanently (checkpointed; can still be resumed manually).
    pub fn stop(&self) {
        self.signal_cancel();
    }

    /// Resume an interrupted scan on a fresh background thread.
    pub fn resume_scan(&self, opts: ScanOptions, sink: Box<dyn EventSink>) -> Result<(), RcError> {
        let spec = self
            .source_spec
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .or_else(|| read_source_id(&self.dir))
            .ok_or_else(|| RcError::not_found("session has no source to resume"))?;
        let (_r, src, info) = resolve::open_source(&spec)?;
        let mut o = opts;
        o.resume = true;
        self.spawn_scan(src, info, o, sink)
    }

    /// Block until the current scan thread finishes, returning its summary.
    /// (For headless drivers/tests; the GUI relies on the `Done` event.)
    pub fn wait(&self) -> Result<Option<ScanSummary>, RcError> {
        let handle = self.scan.lock().ok().and_then(|mut g| g.take());
        match handle {
            Some(h) => match h.join() {
                Ok(res) => res.map(Some),
                Err(_) => Err(RcError::internal("scan thread panicked")),
            },
            None => Ok(None),
        }
    }
}

// --- internal helpers (not exported) ---------------------------------------

impl Session {
    fn spawn_scan(
        &self,
        src: Arc<dyn reclaim_block::BlockSource>,
        info: SourceInfo,
        opts: ScanOptions,
        sink: Box<dyn EventSink>,
    ) -> Result<(), RcError> {
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut g) = self.cancel.lock() {
            *g = Some(Arc::clone(&cancel));
        }
        let dir = self.dir.clone();
        let handle = std::thread::Builder::new()
            .name("reclaim-scan".into())
            .spawn(move || run_pipeline(&dir, &src, &info, &opts, cancel, sink.as_ref()))
            .map_err(|e| RcError::internal(format!("spawn scan thread: {e}")))?;
        if let Ok(mut g) = self.scan.lock() {
            *g = Some(handle);
        }
        Ok(())
    }

    fn signal_cancel(&self) {
        if let Ok(g) = self.cancel.lock() {
            if let Some(c) = g.as_ref() {
                c.store(true, Ordering::SeqCst);
            }
        }
    }

    fn open_store(&self) -> Result<Store, RcError> {
        Store::open(&self.dir.join("session.sqlite"))
            .map_err(|e| RcError::not_found(format!("open session {}: {e}", self.dir.display())))
    }

    fn resolve_source(&self) -> Result<Resolved, RcError> {
        if let Some(spec) = self.source_spec.lock().ok().and_then(|g| g.clone()) {
            return Resolved::parse(&spec);
        }
        let id =
            read_source_id(&self.dir).ok_or_else(|| RcError::not_found("session has no source"))?;
        Resolved::from_source_id(&id)
            .ok_or_else(|| RcError::not_found(format!("cannot reconstruct source from {id}")))
    }

    fn open_reader(&self) -> Result<SourceReader, RcError> {
        let resolved = self.resolve_source()?;
        self.open_reader_for(&resolved)
    }

    fn open_reader_for(&self, resolved: &Resolved) -> Result<SourceReader, RcError> {
        let src = resolved.open()?;
        Ok(SourceReader::new(src))
    }
}

/// The quick → deep → structure → merge pipeline the CLI's `scan` runs.
fn run_pipeline(
    dir: &Path,
    src: &Arc<dyn reclaim_block::BlockSource>,
    info: &SourceInfo,
    opts: &ScanOptions,
    cancel: Arc<AtomicBool>,
    sink: &dyn EventSink,
) -> Result<ScanSummary, RcError> {
    let run_quick = opts.quick || !opts.deep;
    let run_deep = opts.deep || !opts.quick;

    let mut session = if opts.resume && dir.join("session.sqlite").exists() {
        reclaim_session::Session::open(dir, info, false)?
    } else {
        reclaim_session::Session::create(dir, info.clone())?
    };

    let mut on_event = |ev: &reclaim_session::Event| sink.on_event(to_rc_event(ev));

    let mut bitmaps = Vec::new();
    if run_quick {
        let meta = session.run_quick(src, &mut on_event)?;
        bitmaps = meta.bitmaps;
    }

    let mut deep_report: Option<reclaim_session::ScanReport> = None;
    if run_deep {
        let carve = CarveOptions {
            brute_force: opts.brute_force,
            block_size: opts.block_size,
            families: opts.families.clone(),
            sig_ids: opts.sig_ids.clone(),
            keep_corrupted: opts.keep_corrupted,
            max_file_size: opts.max_file_size,
            range: match (opts.range_start, opts.range_end) {
                (Some(a), Some(b)) => Some((a, b)),
                _ => None,
            },
        };
        let cfg = ScanConfig {
            carve,
            resume: opts.resume,
            checkpoint_secs: opts.checkpoint_secs.max(1),
            emit_events: true,
            cancel: Some(Arc::clone(&cancel)),
        };
        let report = session.run_carve(src, &cfg, &mut on_event)?;
        if !bitmaps.is_empty() {
            let _ = session.label_carved(&bitmaps, opts.unallocated_only)?;
        }
        if report.interrupted {
            let total = session.store().count()?;
            return Ok(ScanSummary {
                found: report.found,
                interrupted: true,
                elapsed: report.elapsed,
                complete: false,
                vanished: report.vanished,
                total_rows: total,
            });
        }
        // Structure pass (lost volumes) alongside the deep read.
        let props = reclaim_session::structs::scan_and_store(src, session.store())?;
        for p in &props {
            sink.on_event(RcEvent::Volume {
                start: p.start,
                len: p.len,
                fs: p.fs.clone(),
                confidence: f64::from(p.confidence),
            });
        }
        deep_report = Some(report);
    }

    if run_quick && run_deep {
        let _ = session.merge()?;
    }

    let total = session.store().count()?;
    let (found, elapsed, complete, vanished) = match &deep_report {
        Some(r) => (r.found, r.elapsed, r.complete, r.vanished),
        None => (total, 0.0, true, false),
    };
    Ok(ScanSummary {
        found,
        interrupted: false,
        elapsed,
        complete,
        vanished,
        total_rows: total,
    })
}

/// Read a set of extents (capped) into one buffer for previewing.
fn read_bytes(reader: &SourceReader, extents: &[(u64, u64)], cap: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut budget = cap;
    for (off, len) in extents {
        if budget == 0 {
            break;
        }
        let take = (*len).min(budget);
        let mut o = *off;
        let mut remaining = take;
        while remaining > 0 {
            let n = remaining.min(4 * 1024 * 1024) as usize;
            let buf = reader.read(o, n);
            out.extend_from_slice(&buf);
            o += n as u64;
            remaining -= n as u64;
        }
        budget -= take;
    }
    out
}

/// Read the `source_id` from `source.json`.
fn read_source_id(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("source.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("source_id")
        .and_then(|s| s.as_str())
        .map(str::to_string)
}

/// A default per-source session directory under the user's data location.
fn default_session_dir(source_id: &str) -> PathBuf {
    let h = format!("{:x}", md5_like(source_id));
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Library/Application Support/Reclaim/sessions")
        .join(format!("gui-{h}"))
}

/// A tiny FNV-1a hash for the session-dir name (no crypto needed here).
fn md5_like(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Current UTC timestamp for report metadata.
fn now_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
