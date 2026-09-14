//! Image-container sources (docs/plan/04 §5): DMG UDIF, Apple sparseimage,
//! VMDK, VDI, VHD/VHDX, QCOW2, EnCase E01 and split (`.001`) sets, each exposed
//! as a read-only [`BlockSource`] presenting the *guest* raw image.
//!
//! [`open_container`] sniffs a file and, if it is a known container, returns a
//! source over the reconstructed raw image; otherwise it returns `Ok(None)` and
//! the caller falls back to a plain [`crate::ImageFile`]. Parsing is fully
//! bounds-checked and never panics on hostile input (build guide Part 1.4).

mod codec;
mod concat;
mod dmg;
mod e01;
mod map;
mod qcow2;
mod sparseimage;
mod split;
mod vdi;
mod vhd;
mod vhdx;
mod vmdk;

pub use codec::Codec;
pub use concat::ConcatSource;
pub use map::MappedSource;

use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::BlockSource;
use std::path::Path;
use std::sync::Arc;

/// A recognized container format.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Container {
    /// Raw / dd image (no container) — open with [`crate::ImageFile`].
    Raw,
    /// Apple UDIF disk image (`.dmg`).
    Dmg,
    /// Apple sparse disk image (`.sparseimage`).
    SparseImage,
    /// VMware virtual disk (`.vmdk`), flat or sparse.
    Vmdk,
    /// VirtualBox virtual disk (`.vdi`).
    Vdi,
    /// Microsoft VHD (`.vhd`), fixed or dynamic.
    Vhd,
    /// Microsoft VHDX (`.vhdx`).
    Vhdx,
    /// QEMU copy-on-write v2/v3 (`.qcow2`).
    Qcow2,
    /// EnCase evidence file (`.E01` / `.Ex01`), possibly multi-segment.
    Ewf,
    /// A split raw set (`.001`, `.002`, …).
    Split,
}

impl Container {
    /// Lowercase label for output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Container::Raw => "raw",
            Container::Dmg => "dmg",
            Container::SparseImage => "sparseimage",
            Container::Vmdk => "vmdk",
            Container::Vdi => "vdi",
            Container::Vhd => "vhd",
            Container::Vhdx => "vhdx",
            Container::Qcow2 => "qcow2",
            Container::Ewf => "ewf(e01)",
            Container::Split => "split",
        }
    }
}

/// Sniff `path`'s container format from its header/trailer bytes and name.
/// Cheap: reads at most a few KiB. Returns [`Container::Raw`] for anything
/// unrecognized.
#[must_use]
pub fn detect(path: &Path) -> Container {
    let Ok(img) = ImageFile::open(path) else {
        return Container::Raw;
    };
    let src: Arc<dyn BlockSource> = Arc::new(img);
    detect_source(&src, path)
}

/// Detection over an already-open source (header magics) plus the file name
/// (extension-driven formats: flat VMDK descriptor, split sets).
fn detect_source(src: &Arc<dyn BlockSource>, path: &Path) -> Container {
    let len = src.len();
    let mut head = [0u8; 2048];
    let _ = src.read_at(0, &mut head);

    // QCOW2: "QFI\xfb" big-endian at 0.
    if head.starts_with(&[0x51, 0x46, 0x49, 0xFB]) {
        return Container::Qcow2;
    }
    // VMDK sparse: "KDMV" at 0.
    if head.starts_with(b"KDMV") {
        return Container::Vmdk;
    }
    // VMDK descriptor (flat): text file beginning with the descriptor banner.
    if head.starts_with(b"# Disk DescriptorFile")
        || (head.windows(15).any(|w| w == b"createType=\"vmfs")
            || head.windows(11).any(|w| w == b"createType="))
    {
        return Container::Vmdk;
    }
    // VHDX: "vhdxfile" at 0.
    if head.starts_with(b"vhdxfile") {
        return Container::Vhdx;
    }
    // EWF / EnCase: "EVF\x09\x0d\x0a\xff\x00" or "LVF\x09..." at 0.
    if head.starts_with(b"EVF\x09\x0d\x0a\xff\x00") || head.starts_with(b"LVF\x09\x0d\x0a\xff\x00")
    {
        return Container::Ewf;
    }
    // Apple sparseimage: "sprs" at 0.
    if head.starts_with(b"sprs") {
        return Container::SparseImage;
    }
    // VDI: signature 0xBEDA107F little-endian at 0x40.
    if head.get(0x40..0x44) == Some(&[0x7F, 0x10, 0xDA, 0xBE]) {
        return Container::Vdi;
    }
    // Trailing 512 bytes (read via the aligned-superset helper so unaligned
    // file lengths still yield the true trailer):
    // DMG UDIF "koly" magic / VHD "conectix" footer cookie.
    if len >= 512 {
        if let Some(trailer) = read_exact_vec(src, len - 512, 512) {
            if trailer.starts_with(b"koly") {
                return Container::Dmg;
            }
            if trailer.starts_with(b"conectix") {
                return Container::Vhd;
            }
        }
    }

    // Split raw set: `foo.001` with a sibling `foo.002`.
    if split::looks_like_split(path) {
        return Container::Split;
    }
    Container::Raw
}

/// Open `path` as a container source if it is a recognized image format.
///
/// Returns `Ok(None)` for a plain raw image so the caller can open it with
/// [`crate::ImageFile`] (keeping the raw path allocation-free).
pub fn open_container(path: &Path) -> Result<Option<Arc<dyn BlockSource>>, BlockError> {
    let kind = detect(path);
    let out: Arc<dyn BlockSource> = match kind {
        Container::Raw => return Ok(None),
        Container::Dmg => Arc::new(dmg::open(path)?),
        Container::SparseImage => Arc::new(sparseimage::open(path)?),
        Container::Vmdk => vmdk::open(path)?,
        Container::Vdi => Arc::new(vdi::open(path)?),
        Container::Vhd => vhd::open(path)?,
        Container::Vhdx => Arc::new(vhdx::open(path)?),
        Container::Qcow2 => Arc::new(qcow2::open(path)?),
        Container::Ewf => Arc::new(e01::open(path)?),
        Container::Split => Arc::new(split::open(path)?),
    };
    Ok(Some(out))
}

// -------------------------------------------------------------------------
// Shared bounds-checked byte readers. Containers are untrusted input, so every
// field read is fallible.
// -------------------------------------------------------------------------

#[inline]
pub(crate) fn be_u32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(off..off + 4)?.try_into().ok()?))
}
#[inline]
pub(crate) fn be_u64(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(off..off + 8)?.try_into().ok()?))
}
#[inline]
pub(crate) fn le_u16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}
#[inline]
pub(crate) fn le_u32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}
#[inline]
pub(crate) fn le_u64(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// Read exactly `len` bytes at `off` from a source into a fresh `Vec`, returning
/// `None` if any covered sector is bad/short. Used by parsers to slurp small
/// metadata blocks (headers, tables) with bounds checking.
pub(crate) fn read_exact_vec(src: &Arc<dyn BlockSource>, off: u64, len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return Some(Vec::new());
    }
    // Read an aligned superset (the source requires sector-aligned offsets).
    // Container files (DMG especially) are not sector-length multiples, so clamp
    // the aligned-up end to EOF: reading exactly to a non-aligned EOF leaves no
    // bad sector, whereas reading past it would mark the valid final sector bad.
    let ss = u64::from(src.sector_size().max(1));
    let start = off - (off % ss);
    let end = off.checked_add(len as u64)?;
    if end > src.len() {
        return None; // needed range runs past EOF
    }
    let end_al = core::cmp::min(end.div_ceil(ss).checked_mul(ss)?, src.len());
    let span = usize::try_from(end_al.checked_sub(start)?).ok()?;
    let mut tmp = vec![0u8; span];
    let r = src.read_at(start, &mut tmp);
    // Only sectors covering the needed [off, end) range must be good.
    let first = usize::try_from((off - start) / ss).ok()?;
    let last = usize::try_from((end - 1 - start) / ss).ok()?;
    for i in first..=last {
        if r.status(i) != crate::source::SectorStatus::Good {
            return None;
        }
    }
    let delta = usize::try_from(off - start).ok()?;
    tmp.get(delta..delta + len).map(<[u8]>::to_vec)
}
