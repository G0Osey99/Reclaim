//! Reclaim lost-structure engine (docs/plan/03 §2.4, FR-SCAN-4).
//!
//! Scans a source for partition-table and filesystem **anchors** — the magic
//! numbers and superblocks that survive when a partition table has been wiped or
//! a volume re-formatted — and proposes recoverable volumes. Each hit becomes a
//! [`ProposedVolume`] with `{start, len, fs, confidence, evidence}`; the CLI can
//! `adopt` one and hand it to a metadata engine as an offset window.
//!
//! Covered anchors (doc 04 §1–§2): GPT primary + **backup** header and its
//! entries, MBR, APFS `NXSB`/`APSB`, HFS+ volume header + alternate VH, NTFS
//! boot + `FILE0` MFT-record density, FAT BPB + backup boot sector, exFAT, ext
//! `0xEF53` + group backups, XFS `XFSB`, Btrfs `_BHRfS_M` @64 KiB, ZFS
//! uberblock, UFS, F2FS, ISO 9660 `CD001`, VMFS.
//!
//! Parsing is bounds-checked and never panics on hostile input (build guide
//! Part 1.4 rule 3). The scan reads the source **sequentially** in chunks, so it
//! can ride the same read as the deep carve pass.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

mod anchors;
mod gpt;

use reclaim_block::BlockSource;
use std::sync::Arc;

pub use anchors::FsAnchor;

/// A proposed recoverable volume found by the structure scan.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProposedVolume {
    /// Byte offset of the volume start within the source.
    pub start: u64,
    /// Volume length in bytes (best estimate from the superblock).
    pub len: u64,
    /// Filesystem kind (`"apfs"`, `"ntfs"`, `"ext"`, `"xfs"`, …) or `"gpt-entry"`.
    pub fs: String,
    /// Confidence 0..1 that this is a real, adoptable volume.
    pub confidence: f32,
    /// Human-readable evidence for the proposal.
    pub evidence: String,
}

/// Chunk size for the sequential sweep (1 MiB).
const CHUNK: u64 = 1024 * 1024;
/// Overlap so a magic straddling a chunk boundary is still found.
const OVERLAP: usize = 4096;
/// Hard cap on proposals (DoS guard on a crafted image full of magics).
const MAX_PROPOSALS: usize = 4096;

/// Scan `src` for lost volumes and return de-duplicated proposals, best first.
#[must_use]
pub fn scan(src: &Arc<dyn BlockSource>) -> Vec<ProposedVolume> {
    let mut out: Vec<ProposedVolume> = Vec::new();

    // 1. GPT: primary (LBA 1) and backup (last LBA) headers + their entries.
    gpt::scan_gpt(src, &mut out);

    // 2. Sequential magic sweep for filesystem anchors.
    let total = src.len();
    let anchors = anchors::all();
    let mut chunk_start = 0u64;
    let mut buf = vec![0u8; (CHUNK as usize) + OVERLAP];
    while chunk_start < total && out.len() < MAX_PROPOSALS {
        let want = core::cmp::min(CHUNK + OVERLAP as u64, total - chunk_start) as usize;
        let slice = match buf.get_mut(..want) {
            Some(s) => s,
            None => break,
        };
        let r = src.read_at(chunk_start, slice);
        // Even with bad sectors we scan what we got (zeros where bad).
        let _ = r;
        for a in anchors {
            for rel in memchr::memmem::find_iter(slice, a.needle) {
                let pos = chunk_start + rel as u64;
                // Only act on hits within this chunk's non-overlap region (except
                // the last chunk) to avoid double-counting straddlers.
                if rel >= CHUNK as usize && chunk_start + CHUNK < total {
                    continue;
                }
                if pos < a.internal_offset {
                    continue;
                }
                let vstart = pos - a.internal_offset;
                for mut pv in (a.validate)(src, vstart) {
                    dedup_push(&mut out, &mut pv);
                }
                if out.len() >= MAX_PROPOSALS {
                    break;
                }
            }
        }
        chunk_start += CHUNK;
    }

    // Highest confidence first, then by offset.
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.start.cmp(&b.start))
    });
    out
}

/// Push `pv` unless a same-fs proposal already covers this start (keep the
/// higher confidence).
fn dedup_push(out: &mut Vec<ProposedVolume>, pv: &mut ProposedVolume) {
    for existing in out.iter_mut() {
        if existing.fs == pv.fs && starts_close(existing.start, pv.start) {
            if pv.confidence > existing.confidence {
                *existing = pv.clone();
            }
            return;
        }
    }
    out.push(pv.clone());
}

/// Two starts are "the same volume" if within one sector.
fn starts_close(a: u64, b: u64) -> bool {
    a.max(b) - a.min(b) < 512
}

// -------- bounds-checked readers shared by the anchor validators --------

pub(crate) fn read(src: &Arc<dyn BlockSource>, offset: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let total = src.len();
    if len == 0 || offset >= total {
        return out;
    }
    let ss = u64::from(src.sector_size().max(1));
    let aligned = offset - (offset % ss);
    let end = offset.saturating_add(len as u64).min(total);
    let aligned_end = end.div_ceil(ss).saturating_mul(ss);
    let span = (aligned_end - aligned) as usize;
    let mut buf = vec![0u8; span];
    let _ = src.read_at(aligned, &mut buf);
    let skip = (offset - aligned) as usize;
    let avail = span.saturating_sub(skip);
    let copy = len.min(avail).min((end - offset) as usize);
    if let (Some(d), Some(s)) = (out.get_mut(..copy), buf.get(skip..skip + copy)) {
        d.copy_from_slice(s);
    }
    out
}

#[inline]
pub(crate) fn le_u16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .unwrap_or(0)
}
#[inline]
pub(crate) fn le_u32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .unwrap_or(0)
}
#[inline]
pub(crate) fn le_u64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
        .unwrap_or(0)
}
#[inline]
pub(crate) fn be_u16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_be_bytes)
        .unwrap_or(0)
}
#[inline]
pub(crate) fn be_u32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_be_bytes)
        .unwrap_or(0)
}
#[inline]
pub(crate) fn be_u64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_be_bytes)
        .unwrap_or(0)
}
