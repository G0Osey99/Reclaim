//! Scan orchestration (docs/plan/03 §3): a background carve thread feeds a
//! bounded channel drained by the single SQLite-writer thread, which checkpoints
//! every 5 s or 1 GiB. SIGINT checkpoints and stops (exit 4, resumable); a second
//! SIGINT exits immediately. `--resume` continues from the stored cursor with a
//! fixed block size so the result set is identical (deterministic ids).

use crate::events::Event;
use crate::{Plan, Session, SessionError};
use crossbeam_channel::{bounded, RecvTimeoutError};
use reclaim_block::BlockSource;
use reclaim_carve::engine::{CarveEngine, CarveOptions, CarveProgress};
use reclaim_carve::CarvedFile;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ENGINE: &str = "carve";
const ONE_GIB: u64 = 1024 * 1024 * 1024;
const BATCH: usize = 1024;

static INTERRUPT: AtomicBool = AtomicBool::new(false);
static INT_COUNT: AtomicU8 = AtomicU8::new(0);

extern "C" fn on_sigint(_sig: libc::c_int) {
    let prior = INT_COUNT.fetch_add(1, Ordering::SeqCst);
    if prior >= 1 {
        // Second SIGINT → immediate exit (session may lose ≤ checkpoint interval).
        unsafe { libc::_exit(130) };
    }
    INTERRUPT.store(true, Ordering::SeqCst);
}

fn install_sigint() {
    INT_COUNT.store(0, Ordering::SeqCst);
    INTERRUPT.store(false, Ordering::SeqCst);
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
    }
}

/// Scan configuration.
#[derive(Clone, Debug)]
pub struct ScanConfig {
    /// Carve options (block_size 0 ⇒ infer/plan).
    pub carve: CarveOptions,
    /// Continue the session's interrupted scan.
    pub resume: bool,
    /// Checkpoint interval in seconds.
    pub checkpoint_secs: u64,
    /// Emit NDJSON events to the on_event callback (stdout) as well as the log.
    pub emit_events: bool,
    /// Cancellation flag; when `None` the process SIGINT flag is used. Tests
    /// inject one and flip it from the progress callback.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        ScanConfig {
            carve: CarveOptions::default(),
            resume: false,
            checkpoint_secs: 5,
            emit_events: false,
            cancel: None,
        }
    }
}

/// Report from a scan run.
#[derive(Clone, Debug)]
pub struct ScanReport {
    /// Results found this run.
    pub found: u64,
    /// True if interrupted (session resumable, exit 4).
    pub interrupted: bool,
    /// Elapsed seconds.
    pub elapsed: f64,
    /// Block size used.
    pub block_size: u32,
    /// Whether the scan reached the end (complete).
    pub complete: bool,
}

enum Msg {
    Found(Box<CarvedFile>),
    Progress(CarveProgress),
}

impl Session {
    /// Run the carve engine over `src`, storing results and checkpointing.
    pub fn run_carve(
        &mut self,
        src: &Arc<dyn BlockSource>,
        cfg: &ScanConfig,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<ScanReport, SessionError> {
        let engine = CarveEngine::new();

        // Planner: pick the block size once (fixed across resume).
        let prior = self.store.get_progress(ENGINE)?;
        if let Some((_, true, bs)) = prior {
            // Already complete.
            return Ok(ScanReport {
                found: self.store.count()?,
                interrupted: false,
                elapsed: 0.0,
                block_size: bs,
                complete: true,
            });
        }
        let (resume_from, block_size) = match (cfg.resume, prior) {
            (true, Some((cursor, false, bs))) => (cursor, bs),
            _ => {
                let bs = if cfg.carve.block_size != 0 {
                    cfg.carve.block_size
                } else {
                    engine.infer_block_size(src)
                };
                (0, bs)
            }
        };
        self.write_plan(&Plan {
            engines: vec![ENGINE.to_string()],
            block_size,
        })?;

        let mut opts = cfg.carve.clone();
        opts.block_size = block_size;

        install_sigint();
        let cancel_ref: &AtomicBool = cfg.cancel.as_deref().unwrap_or(&INTERRUPT);
        let start = Instant::now();

        // Open the NDJSON log for append.
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.log_path())?;

        let (tx, rx) = bounded::<Msg>(BATCH * 4);
        let src_scan = Arc::clone(src);
        let opts_scan = opts.clone();

        let report = std::thread::scope(|scope| -> Result<ScanReport, SessionError> {
            let handle = scope.spawn(move || {
                let mut on_found = |cf: CarvedFile| {
                    let _ = tx.send(Msg::Found(Box::new(cf)));
                };
                let mut on_prog = |p: CarveProgress| {
                    let _ = tx.send(Msg::Progress(p));
                };
                engine.scan(
                    &src_scan,
                    &opts_scan,
                    resume_from,
                    &mut on_prog,
                    &mut on_found,
                    cancel_ref,
                )
                // tx dropped here → writer sees disconnect.
            });

            // Writer loop (single writer; owns the SQLite connection).
            let mut pending: Vec<CarvedFile> = Vec::with_capacity(BATCH);
            let mut cur = ProgressState {
                cursor: resume_from,
                scanned: 0,
                total: src.len(),
                found: 0,
            };
            let mut last_ckpt = Instant::now();
            let mut last_ckpt_scanned = 0u64;
            let mut last_rate_t = Instant::now();
            let mut last_rate_scanned = 0u64;
            let ckpt = Duration::from_secs(cfg.checkpoint_secs.max(1));

            loop {
                match rx.recv_timeout(Duration::from_millis(400)) {
                    Ok(Msg::Found(cf)) => {
                        let cf = *cf;
                        let subtype = cf.format.rsplit('.').next().unwrap_or(&cf.format);
                        let ev = Event::Found {
                            id: crate::id::result_id(
                                &self.source.source_id,
                                ENGINE,
                                cf.offset,
                                cf.len,
                            ),
                            engine: ENGINE.to_string(),
                            path: cf
                                .meta
                                .suggested_path(&cf.family, subtype, cf.offset, &cf.ext),
                            size: cf.len,
                            score: cf.score,
                        };
                        write_event(&mut log, &ev, cfg.emit_events, on_event);
                        pending.push(cf);
                        if pending.len() >= BATCH {
                            flush(self, &mut pending)?;
                        }
                    }
                    Ok(Msg::Progress(p)) => {
                        cur.cursor = p.cursor;
                        cur.scanned = p.scanned;
                        cur.total = p.total;
                        cur.found = p.found;
                        let dt = last_rate_t.elapsed().as_secs_f64();
                        let rate = if dt > 0.0 {
                            ((p.scanned - last_rate_scanned) as f64 / dt) as u64
                        } else {
                            0
                        };
                        last_rate_t = Instant::now();
                        last_rate_scanned = p.scanned;
                        let pct = if cur.total > 0 {
                            cur.scanned as f64 / cur.total as f64 * 100.0
                        } else {
                            0.0
                        };
                        let ev = Event::Progress {
                            pass: ENGINE.to_string(),
                            lba: p.cursor,
                            pct,
                            rate,
                        };
                        write_event(&mut log, &ev, cfg.emit_events, on_event);
                        if last_ckpt.elapsed() >= ckpt
                            || cur.scanned.saturating_sub(last_ckpt_scanned) >= ONE_GIB
                        {
                            flush(self, &mut pending)?;
                            self.store.put_progress(
                                ENGINE,
                                cur.cursor,
                                cur.scanned,
                                cur.total,
                                cur.found,
                                false,
                                block_size,
                            )?;
                            let _ = self.store.put_event("progress", &ev.to_ndjson());
                            last_ckpt = Instant::now();
                            last_ckpt_scanned = cur.scanned;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if last_ckpt.elapsed() >= ckpt {
                            flush(self, &mut pending)?;
                            self.store.put_progress(
                                ENGINE,
                                cur.cursor,
                                cur.scanned,
                                cur.total,
                                cur.found,
                                false,
                                block_size,
                            )?;
                            last_ckpt = Instant::now();
                            last_ckpt_scanned = cur.scanned;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            let outcome = handle
                .join()
                .map_err(|_| SessionError::Other("scan thread panicked".into()))?;

            flush(self, &mut pending)?;
            let complete = !outcome.interrupted;
            self.store.put_progress(
                ENGINE,
                outcome.cursor,
                cur.total.min(cur.scanned.max(outcome.cursor)),
                cur.total,
                outcome.found,
                complete,
                block_size,
            )?;
            let elapsed = start.elapsed().as_secs_f64();
            let done = Event::Done {
                found: outcome.found,
                elapsed,
                interrupted: outcome.interrupted,
            };
            write_event(&mut log, &done, cfg.emit_events, on_event);
            let _ = self.store.put_event("done", &done.to_ndjson());

            Ok(ScanReport {
                found: outcome.found,
                interrupted: outcome.interrupted,
                elapsed,
                block_size,
                complete,
            })
        })?;

        Ok(report)
    }
}

struct ProgressState {
    cursor: u64,
    scanned: u64,
    total: u64,
    found: u64,
}

fn flush(session: &mut Session, pending: &mut Vec<CarvedFile>) -> Result<(), SessionError> {
    if pending.is_empty() {
        return Ok(());
    }
    let sid = session.source.source_id.clone();
    session.store.insert_carved(&sid, ENGINE, pending)?;
    pending.clear();
    Ok(())
}

fn write_event(log: &mut std::fs::File, ev: &Event, emit: bool, on_event: &mut dyn FnMut(&Event)) {
    let line = ev.to_ndjson();
    let _ = writeln!(log, "{line}");
    if emit {
        on_event(ev);
    }
}
