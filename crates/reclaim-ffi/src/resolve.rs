//! Source-string resolution for the FFI, lifted from the CLI's `source.rs` and
//! `common.rs` (the agent-mapped "three CLI-only pieces"). It depends only on
//! library crates, so the GUI resolves `diskN` / image paths / adopted-volume
//! specs exactly as the CLI does (docs/plan/07 §1), plus the same-whole-disk
//! recover refusal and a free-space check (docs/plan/06 §10).

use crate::RcError;
use reclaim_block::{BlockSource, OffsetView, RawDevice};
use reclaim_platform_macos::Disk;
use reclaim_session::SourceInfo;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A resolved source specification (mirror of `reclaim_cli::source::Resolved`).
#[derive(Debug, Clone)]
pub(crate) enum Resolved {
    /// A raw device identified by BSD name.
    Device {
        bsd: String,
        raw_node: String,
        disk: Option<Box<Disk>>,
    },
    /// An image file (raw or an auto-detected container).
    Image { path: PathBuf },
    /// An adopted lost-structure volume: proposal `index` (1-based) in a session.
    Volume { session_dir: PathBuf, index: usize },
    /// A raw file descriptor handed over by the privileged helper (doc 03 §7),
    /// spelt `fd:<n>` or `fd:<n>:<bsd-label>`.
    Fd { fd: i32, label: String },
}

impl Resolved {
    /// Resolve a `SOURCE` string (`session:<dir>/volume/<n>`, `diskN`,
    /// `/dev/[r]diskN`, or an image path).
    pub(crate) fn parse(spec: &str) -> Result<Resolved, RcError> {
        if let Some(rest) = spec.strip_prefix("session:") {
            let (dir, n) = rest.rsplit_once("/volume/").ok_or_else(|| {
                RcError::usage(format!(
                    "bad volume spec '{spec}': expected session:<dir>/volume/<n>"
                ))
            })?;
            let index: usize = n
                .parse()
                .map_err(|_| RcError::usage(format!("bad volume index '{n}' in '{spec}'")))?;
            return Ok(Resolved::Volume {
                session_dir: PathBuf::from(dir),
                index,
            });
        }
        if let Some(rest) = spec.strip_prefix("fd:") {
            let (num, label) = match rest.split_once(':') {
                Some((n, l)) => (n, l.to_string()),
                None => (rest, "fd".to_string()),
            };
            let fd: i32 = num
                .parse()
                .map_err(|_| RcError::usage(format!("bad fd '{num}' in '{spec}'")))?;
            return Ok(Resolved::Fd { fd, label });
        }
        let stripped = spec.strip_prefix("/dev/").unwrap_or(spec);
        let bsd_candidate = stripped.strip_prefix('r').unwrap_or(stripped);
        let looks_like_device = bsd_candidate.starts_with("disk")
            && bsd_candidate
                .strip_prefix("disk")
                .and_then(|r| r.chars().next())
                .is_some_and(|c| c.is_ascii_digit());
        if looks_like_device {
            let bsd = bsd_candidate.to_string();
            let raw_node = format!("/dev/r{bsd}");
            let disk = reclaim_platform_macos::disk_by_bsd(&bsd)
                .ok()
                .flatten()
                .map(Box::new);
            return Ok(Resolved::Device {
                bsd,
                raw_node,
                disk,
            });
        }
        let path = Path::new(spec);
        if path.is_file() {
            return Ok(Resolved::Image {
                path: path.to_path_buf(),
            });
        }
        Err(RcError::not_found(format!(
            "source '{spec}' is neither a known device (diskN) nor an existing image file"
        )))
    }

    /// Reconstruct a source from a persisted `source_id`.
    pub(crate) fn from_source_id(id: &str) -> Option<Resolved> {
        if let Some(rest) = id.strip_prefix("image:") {
            let path = rest.rsplit_once(':').map(|(p, _)| p).unwrap_or(rest);
            return Some(Resolved::Image {
                path: PathBuf::from(path),
            });
        }
        if let Some(rest) = id.strip_prefix("device:") {
            let bsd = rest.rsplit_once(':').map(|(b, _)| b).unwrap_or(rest);
            return Some(Resolved::Device {
                bsd: bsd.to_string(),
                raw_node: format!("/dev/r{bsd}"),
                disk: None,
            });
        }
        for pfx in [
            "dmg:",
            "vmdk:",
            "vmdk-flat:",
            "vdi:",
            "vhd-fixed:",
            "vhd-dyn:",
            "vhdx:",
            "qcow2:",
            "ewf:",
            "sparseimage:",
            "split:",
        ] {
            if let Some(rest) = id.strip_prefix(pfx) {
                let path = rest.rsplit_once(':').map(|(p, _)| p).unwrap_or(rest);
                return Some(Resolved::Image {
                    path: PathBuf::from(path),
                });
            }
        }
        None
    }

    /// Open the source read-only.
    pub(crate) fn open(&self) -> Result<Arc<dyn BlockSource>, RcError> {
        match self {
            Resolved::Device { raw_node, bsd, .. } => match RawDevice::open(raw_node) {
                Ok(dev) => Ok(Arc::new(dev)),
                Err(e) => Err(map_open_error(bsd, raw_node, &e)),
            },
            Resolved::Image { path } => match reclaim_block::open_image_auto(path) {
                Ok(src) => Ok(src),
                Err(e) => Err(RcError::not_found(format!(
                    "cannot open image '{}': {e}",
                    path.display()
                ))),
            },
            Resolved::Volume { session_dir, index } => open_adopted(session_dir, *index),
            Resolved::Fd { fd, label } => RawDevice::from_fd(*fd, label)
                .map(|d| Arc::new(d) as Arc<dyn BlockSource>)
                .map_err(|e| RcError::internal(format!("adopt fd {fd}: {e}"))),
        }
    }

    /// The original source behind an adopted volume (for the same-disk check).
    pub(crate) fn underlying(&self) -> Option<Resolved> {
        match self {
            Resolved::Volume { session_dir, .. } => {
                let info = reclaim_session::Session::read_source_info(session_dir).ok()?;
                Resolved::from_source_id(&info.source_id)
            }
            _ => None,
        }
    }
}

/// Open + build a [`SourceInfo`] (mirror of the CLI's `open_source`).
pub(crate) fn open_source(
    spec: &str,
) -> Result<(Resolved, Arc<dyn BlockSource>, SourceInfo), RcError> {
    let resolved = Resolved::parse(spec)?;
    let src = resolved.open()?;
    let (first_mib_hash, last_mib_hash) = reclaim_session::source_hashes(&src);
    let info = SourceInfo {
        source_id: src.id().as_str().to_string(),
        size: src.len(),
        sector_size: src.sector_size(),
        first_mib_hash,
        last_mib_hash,
    };
    Ok((resolved, src, info))
}

fn open_adopted(session_dir: &Path, index: usize) -> Result<Arc<dyn BlockSource>, RcError> {
    let info = reclaim_session::Session::read_source_info(session_dir).map_err(|e| {
        RcError::not_found(format!(
            "cannot read session '{}': {e}",
            session_dir.display()
        ))
    })?;
    let store = reclaim_session::Store::open(&session_dir.join("session.sqlite"))
        .map_err(|e| RcError::not_found(format!("cannot open session store: {e}")))?;
    let proposals = store
        .list_proposals()
        .map_err(|e| RcError::internal(format!("reading proposals: {e}")))?;
    if index == 0 || index > proposals.len() {
        return Err(RcError::not_found(format!(
            "no adopted volume {index} (session has {} proposal(s))",
            proposals.len()
        )));
    }
    let p = proposals
        .get(index - 1)
        .ok_or_else(|| RcError::not_found(format!("no proposal {index}")))?;
    if p.len == 0 {
        return Err(RcError::usage(format!(
            "proposal {index} ({}) has no adoptable length",
            p.fs
        )));
    }
    let original = Resolved::from_source_id(&info.source_id)
        .ok_or_else(|| RcError::not_found(format!("cannot reopen source '{}'", info.source_id)))?;
    let base = original.open()?;
    let view = OffsetView::new(base, p.start, p.len)
        .map_err(|e| RcError::internal(format!("adopted volume window invalid: {e}")))?;
    Ok(Arc::new(view))
}

fn map_open_error(bsd: &str, raw_node: &str, err: &reclaim_block::BlockError) -> RcError {
    let msg = err.to_string();
    let is_perm = msg.contains("Permission denied")
        || msg.contains("Operation not permitted")
        || msg.contains("(os error 1)")
        || msg.contains("(os error 13)");
    if is_perm {
        RcError::permission(format!(
            "permission denied opening {raw_node}. Raw device reads need the privileged \
             helper (and Full Disk Access for the boot disk)."
        ))
    } else {
        RcError::not_found(format!("cannot open device {bsd} ({raw_node}): {err}"))
    }
}

// --- Recover-destination safety (docs/plan/06 §10) -------------------------

/// True if recovering to `dest` would write to the same whole disk as `source`.
pub(crate) fn dest_on_same_disk(source: &Resolved, dest: &Path) -> bool {
    match source {
        Resolved::Image { path } => match (fs_device(path), fs_device_ancestor(dest)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
        Resolved::Device { bsd, .. } | Resolved::Fd { label: bsd, .. } => {
            match (Some(whole_disk(bsd)), dest_whole_disk(dest)) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        }
        Resolved::Volume { .. } => source
            .underlying()
            .is_some_and(|inner| dest_on_same_disk(&inner, dest)),
    }
}

/// Bytes free on the filesystem backing `dest` (nearest existing ancestor), via
/// `statvfs`. `None` if it cannot be determined.
pub(crate) fn free_space(dest: &Path) -> Option<u64> {
    let anc = nearest_existing(dest)?;
    let cpath = std::ffi::CString::new(anc.as_os_str().as_encoded_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

fn fs_device(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

fn fs_device_ancestor(path: &Path) -> Option<u64> {
    let mut p = path;
    loop {
        if let Some(d) = fs_device(p) {
            return Some(d);
        }
        p = p.parent()?;
    }
}

fn whole_disk(bsd: &str) -> String {
    if let Some(rest) = bsd.strip_prefix("disk") {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return format!("disk{digits}");
        }
    }
    bsd.to_string()
}

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
    let name = dev.rsplit('/').next().unwrap_or(&dev);
    Some(whole_disk(name))
}

#[cfg(not(target_os = "macos"))]
fn dest_whole_disk(_dest: &Path) -> Option<String> {
    None
}

fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut p = path;
    loop {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        p = p.parent()?;
    }
}
