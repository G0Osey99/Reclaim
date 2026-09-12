//! Open-audit log powering `reclaim --prove-readonly` (docs/plan/07 §5.3,
//! docs/plan/09 §5).
//!
//! Every source open records its path and the `open(2)` flags here. With
//! prove-readonly enabled, the CLI prints the log so an auditor can confirm no
//! source was ever opened with a write flag. [`OpenRecord::write_intent`] flags
//! any open that carried `O_WRONLY`/`O_RDWR`/`O_CREAT`/`O_TRUNC`/`O_APPEND`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// One recorded `open()` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRecord {
    /// Path passed to `open()`.
    pub path: PathBuf,
    /// Raw `open(2)` flags.
    pub flags: i32,
    /// True if the flags include any write/create/truncate/append intent.
    pub write_intent: bool,
}

static PROVE: AtomicBool = AtomicBool::new(false);
static LOG: Mutex<Vec<OpenRecord>> = Mutex::new(Vec::new());

/// Enable/disable prove-readonly recording.
pub fn set_prove_readonly(on: bool) {
    PROVE.store(on, Ordering::SeqCst);
}

/// Whether prove-readonly recording is enabled.
#[must_use]
pub fn is_prove_readonly() -> bool {
    PROVE.load(Ordering::SeqCst)
}

fn has_write_intent(flags: i32) -> bool {
    // O_RDONLY is 0; any of these bits means a non-read-only intent.
    let write_bits = libc::O_WRONLY | libc::O_RDWR | libc::O_CREAT | libc::O_TRUNC | libc::O_APPEND;
    (flags & write_bits) != 0
}

/// Record an `open()` of `path` with `flags`. Cheap no-op unless prove-readonly
/// is enabled.
pub fn record_open(path: &Path, flags: i32) {
    if !is_prove_readonly() {
        return;
    }
    let record = OpenRecord {
        path: path.to_path_buf(),
        flags,
        write_intent: has_write_intent(flags),
    };
    let mut guard = LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.push(record);
}

/// Snapshot the current log without clearing it.
#[must_use]
pub fn snapshot() -> Vec<OpenRecord> {
    let guard = LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clone()
}

/// Take and clear the log.
#[must_use]
pub fn drain() -> Vec<OpenRecord> {
    let mut guard = LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::take(&mut *guard)
}

/// True if any recorded open carried write intent (used by read-only tests).
#[must_use]
pub fn any_write_intent() -> bool {
    let guard = LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.iter().any(|r| r.write_intent)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn records_only_when_enabled() {
        // Note: global state; keep this test self-contained and serialized by
        // draining at the start.
        set_prove_readonly(false);
        let _ = drain();
        record_open(Path::new("/dev/rdisk9"), libc::O_RDONLY);
        assert!(snapshot().is_empty(), "should not record while disabled");

        set_prove_readonly(true);
        record_open(Path::new("/dev/rdisk9"), libc::O_RDONLY);
        let log = drain();
        assert_eq!(log.len(), 1);
        assert!(!log[0].write_intent);
        set_prove_readonly(false);
    }

    #[test]
    fn write_intent_detection() {
        assert!(!has_write_intent(libc::O_RDONLY));
        assert!(has_write_intent(libc::O_WRONLY));
        assert!(has_write_intent(libc::O_RDWR));
        assert!(has_write_intent(libc::O_RDONLY | libc::O_CREAT));
    }
}
