//! Shared helpers for the scan/results/recover/report/preview/image commands:
//! opening a source into a `SourceInfo`, resolving the session directory, and
//! the destination same-whole-disk safety check (docs/plan/07 §5).

use crate::exit::CmdError;
use crate::source::Resolved;
use reclaim_block::BlockSource;
use reclaim_session::SourceInfo;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resolve and open a `SOURCE`, returning the resolution, the opened block
/// source, and its persisted identity.
pub fn open_source(spec: &str) -> Result<(Resolved, Arc<dyn BlockSource>, SourceInfo), CmdError> {
    let resolved = Resolved::parse(spec)?;
    let src = resolved.open()?;
    let info = SourceInfo {
        source_id: src.id().as_str().to_string(),
        size: src.len(),
        sector_size: src.sector_size(),
    };
    Ok((resolved, src, info))
}

/// Resolve the session directory from a positional arg or the global
/// `--session`, defaulting to a per-user data location.
pub fn resolve_session(
    positional: Option<&Path>,
    global: Option<&Path>,
) -> Result<PathBuf, CmdError> {
    if let Some(p) = positional {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = global {
        return Ok(p.to_path_buf());
    }
    Ok(default_session_dir())
}

/// Default session directory (docs/plan/07 §1).
#[must_use]
pub fn default_session_dir() -> PathBuf {
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    let dir = base
        .join("Library/Application Support/Reclaim/sessions")
        .join(timestamp_name());
    #[cfg(not(target_os = "macos"))]
    let dir = base
        .join(".local/share/reclaim/sessions")
        .join(timestamp_name());
    dir
}

/// A stable session directory keyed by the source identity, so `volumes` and a
/// later `adopt`/`scan` (without an explicit `--session`) reuse the same stored
/// proposals (docs/plan/07 §1).
#[must_use]
pub fn stable_session_dir(source_id: &str) -> PathBuf {
    let h = blake3::hash(source_id.as_bytes());
    let short = &h.to_hex()[..16];
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    let dir = base.join("Library/Application Support/Reclaim/sessions");
    #[cfg(not(target_os = "macos"))]
    let dir = base.join(".local/share/reclaim/sessions");
    dir.join(format!("vol-{short}"))
}

fn timestamp_name() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("session-{now}")
}

/// True if recovering to `dest` would write to the same whole disk as the
/// source (a data-loss hazard). Image sources compare by filesystem device;
/// raw-device sources compare by whole-disk BSD name (macOS).
#[must_use]
pub fn dest_on_same_disk(source: &Resolved, dest: &Path) -> bool {
    match source {
        Resolved::Image { path } => match (fs_device(path), fs_device_ancestor(dest)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
        Resolved::Device { bsd, .. } => match (Some(whole_disk(bsd)), dest_whole_disk(dest)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
        // An adopted volume sits on its original source — check that.
        Resolved::Volume { .. } => source
            .underlying()
            .is_some_and(|inner| dest_on_same_disk(&inner, dest)),
    }
}

/// st_dev of a file/dir.
fn fs_device(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

/// st_dev of the nearest existing ancestor of `path`.
fn fs_device_ancestor(path: &Path) -> Option<u64> {
    let mut p = path;
    loop {
        if let Some(d) = fs_device(p) {
            return Some(d);
        }
        {
            let par = p.parent()?;
            p = par
        }
    }
}

/// Whole-disk BSD name for a device (e.g. `disk4s1` → `disk4`).
fn whole_disk(bsd: &str) -> String {
    // Strip a trailing `sN[sM]` slice suffix after the leading `diskN`.
    if let Some(rest) = bsd.strip_prefix("disk") {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return format!("disk{digits}");
        }
    }
    bsd.to_string()
}

/// Whole-disk BSD name backing `dest`'s filesystem (macOS via statfs).
#[cfg(target_os = "macos")]
fn dest_whole_disk(dest: &Path) -> Option<String> {
    let anc = nearest_existing(dest)?;
    let cpath = std::ffi::CString::new(anc.as_os_str().as_encoded_bytes()).ok()?;
    let mut sfs: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(cpath.as_ptr(), &mut sfs) };
    if rc != 0 {
        return None;
    }
    let raw: Vec<u8> = sfs
        .f_mntfromname
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    let dev = String::from_utf8_lossy(&raw).to_string();
    // e.g. "/dev/disk3s5" → "disk3".
    let name = dev.rsplit('/').next().unwrap_or(&dev);
    Some(whole_disk(name))
}

#[cfg(not(target_os = "macos"))]
fn dest_whole_disk(_dest: &Path) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut p = path;
    loop {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        {
            let par = p.parent()?;
            p = par
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_disk_strips_slice() {
        assert_eq!(whole_disk("disk4s1"), "disk4");
        assert_eq!(whole_disk("disk4"), "disk4");
        assert_eq!(whole_disk("disk10s2s1"), "disk10");
    }

    #[test]
    fn image_same_fs_detected() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut tf = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        tf.write_all(b"hello").unwrap();
        tf.flush().unwrap();
        let src = Resolved::Image {
            path: tf.path().to_path_buf(),
        };
        // Destination in the same tempdir → same filesystem device.
        assert!(dest_on_same_disk(&src, &dir.path().join("out")));
    }
}
