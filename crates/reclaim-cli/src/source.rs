//! Resolve a `SOURCE` argument (docs/plan/07 §1) into something readable.
//!
//! Phase 0 understands the two source kinds it can act on now: a raw device
//! (`disk4`, `/dev/rdisk4`, `/dev/disk4`) and an image file (`/path/x.img`).
//! Adopted volumes, RAID and `mounted:` specs arrive in later phases.

use crate::exit::CmdError;
use reclaim_block::{BlockSource, OffsetView, RawDevice};
use reclaim_platform_macos::Disk;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A resolved source specification.
#[derive(Debug)]
pub enum Resolved {
    /// A raw device identified by BSD name, with enumeration metadata if found.
    Device {
        /// BSD name, e.g. `disk4`.
        bsd: String,
        /// Raw node to open, e.g. `/dev/rdisk4`.
        raw_node: String,
        /// Enumeration metadata (None if the device isn't in the inventory).
        disk: Option<Box<Disk>>,
    },
    /// An image file on disk (raw or an auto-detected container).
    Image {
        /// Path to the image.
        path: PathBuf,
    },
    /// An adopted lost-structure volume: proposal `index` (1-based) in the
    /// session at `session_dir`, presented as an [`OffsetView`] over the
    /// original source (docs/plan/07 §1 `session:<id>/volume/<n>`).
    Volume {
        /// The session directory holding the stored proposals + source.json.
        session_dir: PathBuf,
        /// 1-based proposal index.
        index: usize,
    },
}

impl Resolved {
    /// Resolve a `SOURCE` string.
    pub fn parse(spec: &str) -> Result<Resolved, CmdError> {
        // Adopted lost-structure volume: `session:<dir>/volume/<n>`.
        if let Some(rest) = spec.strip_prefix("session:") {
            let (dir, n) = rest.rsplit_once("/volume/").ok_or_else(|| {
                CmdError::new(
                    crate::exit::Exit::Usage,
                    format!("bad volume spec '{spec}': expected session:<dir>/volume/<n>"),
                )
            })?;
            let index: usize = n.parse().map_err(|_| {
                CmdError::new(
                    crate::exit::Exit::Usage,
                    format!("bad volume index '{n}' in '{spec}'"),
                )
            })?;
            return Ok(Resolved::Volume {
                session_dir: PathBuf::from(dir),
                index,
            });
        }
        // Device forms: "diskN", "diskNsM", "/dev/diskN", "/dev/rdiskN".
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

        // Otherwise treat as an image file path.
        let path = Path::new(spec);
        if path.is_file() {
            return Ok(Resolved::Image {
                path: path.to_path_buf(),
            });
        }
        Err(CmdError::not_found(format!(
            "source '{spec}' is neither a known device (diskN) nor an existing image file"
        )))
    }

    /// Reconstruct a source from a persisted `source_id`
    /// (`image:{path}:{len}` or `device:{bsd}:{len}`) so a session can reopen
    /// its source without re-specifying it (docs/plan/07 §4).
    pub fn from_source_id(id: &str) -> Option<Resolved> {
        if let Some(rest) = id.strip_prefix("image:") {
            // Strip the trailing `:{len}`.
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
        // Image-container source ids are `<fmt>:{path}:{len}` (docs/plan/04 §5);
        // reopen by path so `open()` re-detects the container on resume.
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

    /// Open the source read-only as a [`BlockSource`], mapping permission
    /// failures to a clear message (exit 2).
    pub fn open(&self) -> Result<Arc<dyn BlockSource>, CmdError> {
        match self {
            Resolved::Device { raw_node, bsd, .. } => match RawDevice::open(raw_node) {
                Ok(dev) => Ok(Arc::new(dev)),
                Err(e) => Err(map_open_error(bsd, raw_node, e)),
            },
            // Auto-detect image-container formats (DMG/VMDK/VDI/VHD/VHDX/QCOW2/
            // E01/sparseimage/split) and fall back to a raw image (docs/plan/04 §5).
            Resolved::Image { path } => match reclaim_block::open_image_auto(path) {
                Ok(src) => Ok(src),
                Err(e) => Err(CmdError::not_found(format!(
                    "cannot open image '{}': {e}",
                    path.display()
                ))),
            },
            Resolved::Volume { session_dir, index } => open_adopted(session_dir, *index),
        }
    }

    /// The original source behind an adopted `Volume` (for the recover
    /// same-disk safety check); `None` for non-volume sources.
    #[must_use]
    pub fn underlying(&self) -> Option<Resolved> {
        match self {
            Resolved::Volume { session_dir, .. } => {
                let info = reclaim_session::Session::read_source_info(session_dir).ok()?;
                Resolved::from_source_id(&info.source_id)
            }
            _ => None,
        }
    }
}

/// Open proposal `index` (1-based) from the session at `session_dir` as an
/// [`OffsetView`] over the reopened original source.
fn open_adopted(session_dir: &Path, index: usize) -> Result<Arc<dyn BlockSource>, CmdError> {
    let info = reclaim_session::Session::read_source_info(session_dir).map_err(|e| {
        CmdError::not_found(format!(
            "cannot read session '{}': {e}",
            session_dir.display()
        ))
    })?;
    let store = reclaim_session::Store::open(&session_dir.join("session.sqlite"))
        .map_err(|e| CmdError::not_found(format!("cannot open session store: {e}")))?;
    let proposals = store
        .list_proposals()
        .map_err(|e| CmdError::internal(format!("reading proposals: {e}")))?;
    if index == 0 || index > proposals.len() {
        return Err(CmdError::not_found(format!(
            "no adopted volume {index} (session has {} proposal(s))",
            proposals.len()
        )));
    }
    let p = proposals
        .get(index - 1)
        .ok_or_else(|| CmdError::not_found(format!("no proposal {index}")))?;
    if p.len == 0 {
        return Err(CmdError::new(
            crate::exit::Exit::Usage,
            format!(
                "proposal {index} ({}) has no adoptable length; adopt its container instead",
                p.fs
            ),
        ));
    }
    let original = Resolved::from_source_id(&info.source_id)
        .ok_or_else(|| CmdError::not_found(format!("cannot reopen source '{}'", info.source_id)))?;
    let base = original.open()?;
    let view = OffsetView::new(base, p.start, p.len)
        .map_err(|e| CmdError::internal(format!("adopted volume window invalid: {e}")))?;
    Ok(Arc::new(view))
}

fn map_open_error(bsd: &str, raw_node: &str, err: reclaim_block::BlockError) -> CmdError {
    let msg = err.to_string();
    let is_perm = msg.contains("Permission denied")
        || msg.contains("Operation not permitted")
        || msg.contains("(os error 1)")
        || msg.contains("(os error 13)");
    if is_perm {
        CmdError::permission(format!(
            "permission denied opening {raw_node}. Raw device reads need root: try `sudo reclaim …`. \
             For the boot disk you also need Full Disk Access (see `reclaim doctor`)."
        ))
    } else {
        CmdError::not_found(format!("cannot open device {bsd} ({raw_node}): {err}"))
    }
}
