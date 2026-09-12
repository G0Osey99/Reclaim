//! Split raw image sets (`image.001`, `image.002`, …) — docs/plan/04 §5.
//!
//! A dd/`split` capture chopped into numbered pieces. We concatenate `.001`
//! onward (in numeric order) until a piece is missing.

use super::concat::ConcatSource;
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::{BlockSource, SourceId};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// True if `path` is the first piece (`*.001`) of a split set with a `.002`
/// sibling present.
#[must_use]
pub(crate) fn looks_like_split(path: &Path) -> bool {
    if !ends_with_num(path, 1) {
        return false;
    }
    numbered_sibling(path, 2).is_some_and(|p| p.is_file())
}

/// The path with its trailing `.NNN` replaced by `.{n:03}`.
fn numbered_sibling(path: &Path, n: u32) -> Option<PathBuf> {
    let s = path.to_str()?;
    let dot = s.rfind('.')?;
    let stem = s.get(..dot)?;
    Some(PathBuf::from(format!("{stem}.{n:03}")))
}

/// True if `path`'s extension is the 3-digit number `n`.
fn ends_with_num(path: &Path, n: u32) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(|e| e.parse::<u32>().ok())
        .is_some_and(|v| v == n && path.extension().map_or(0, |e| e.len()) == 3)
}

/// Open a split set as one concatenated raw image.
pub(crate) fn open(path: &Path) -> Result<ConcatSource, BlockError> {
    let mut parts: Vec<Arc<dyn BlockSource>> = Vec::new();
    let mut n = 1u32;
    loop {
        let piece = if n == 1 {
            path.to_path_buf()
        } else {
            match numbered_sibling(path, n) {
                Some(p) if p.is_file() => p,
                _ => break,
            }
        };
        let img = ImageFile::open(&piece)?;
        parts.push(Arc::new(img));
        n += 1;
        if n > 100_000 {
            break; // sanity bound
        }
    }
    if parts.is_empty() {
        return Err(BlockError::Container("split: no pieces".into()));
    }
    let id = SourceId::new(format!("split:{}:{}", path.display(), parts.len()));
    Ok(ConcatSource::new(parts, id))
}
