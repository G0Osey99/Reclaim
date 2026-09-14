//! `start_image` + `ImageJob` — the byte-to-byte imager (doc 08 §2.5). Drives
//! `reclaim_block::imaging::image` (the allow-listed writer) on a background
//! thread, streaming a live bad-sector count to the [`crate::EventSink`] so the
//! GUI can paint its bad-sector strip.

use crate::ffitypes::{ImageDone, ImageOptions, RcEvent};
use crate::resolve;
use crate::{EventSink, RcError};
use reclaim_block::imaging::{image, Compression, HashAlgo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A running (or finished) imaging job.
#[derive(uniffi::Object)]
pub struct ImageJob {
    cancel: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<Result<ImageDone, RcError>>>>,
}

impl std::fmt::Debug for ImageJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageJob").finish()
    }
}

/// Start imaging `source` to `dest` (returns immediately; runs on a thread).
#[uniffi::export]
pub fn start_image(
    source: String,
    dest: String,
    opts: ImageOptions,
    sink: Box<dyn EventSink>,
) -> Result<Arc<ImageJob>, RcError> {
    let (resolved, src, _info) = resolve::open_source(&source)?;
    let out = PathBuf::from(&dest);
    if resolve::dest_on_same_disk(&resolved, &out) {
        return Err(RcError::refused(format!(
            "image destination {dest} is on the same disk as the source — refusing (data-loss risk)"
        )));
    }
    let map_path = default_map_path(&out);
    let hash_algo = HashAlgo::parse(&opts.hash)
        .ok_or_else(|| RcError::usage(format!("bad hash '{}'", opts.hash)))?;
    let core_opts = reclaim_block::imaging::ImageOptions {
        chunk_size: if opts.chunk_size == 0 {
            reclaim_block::imaging::imager::DEFAULT_CHUNK
        } else {
            opts.chunk_size
        },
        retries: opts.retries,
        reverse_pass: opts.reverse_pass,
        sparse: opts.sparse && !opts.zstd,
        compression: if opts.zstd {
            Compression::Zstd
        } else {
            Compression::None
        },
        hash_algo,
        resume: opts.resume,
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);
    let out_thread = out.clone();
    let map_thread = map_path.clone();
    let handle = std::thread::Builder::new()
        .name("reclaim-image".into())
        .spawn(move || {
            let mut on_progress = |p: reclaim_block::imaging::ImageProgress| {
                sink.on_event(RcEvent::ImageProgress {
                    pass: p.pass,
                    done: p.done,
                    total: p.total,
                    bad_sectors: p.bad_sectors,
                });
            };
            let outcome = image(
                &src,
                &out_thread,
                &map_thread,
                &core_opts,
                &mut on_progress,
                &cancel_thread,
            )
            .map_err(|e| RcError::internal(format!("imaging failed: {e}")))?;
            Ok(ImageDone {
                complete: outcome.complete,
                had_bad: outcome.had_bad,
                bad_sectors: outcome.bad_sectors,
                whole_hash: outcome.whole_hash,
                image_path: out_thread.to_string_lossy().to_string(),
                map_path: map_thread.to_string_lossy().to_string(),
            })
        })
        .map_err(|e| RcError::internal(format!("spawn image thread: {e}")))?;

    Ok(Arc::new(ImageJob {
        cancel,
        thread: Mutex::new(Some(handle)),
    }))
}

#[uniffi::export]
impl ImageJob {
    /// Cancel the imaging job (checkpointed; resumable with `resume`).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// True while the imaging thread is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.thread
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|h| !h.is_finished()))
            .unwrap_or(false)
    }

    /// Block until imaging finishes and return the outcome.
    pub fn wait(&self) -> Result<ImageDone, RcError> {
        let handle = self.thread.lock().ok().and_then(|mut g| g.take());
        match handle {
            Some(h) => match h.join() {
                Ok(res) => res,
                Err(_) => Err(RcError::internal("image thread panicked")),
            },
            None => Err(RcError::internal("image job already awaited")),
        }
    }
}

fn default_map_path(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_os_string();
    s.push(".reclaim-map");
    PathBuf::from(s)
}
