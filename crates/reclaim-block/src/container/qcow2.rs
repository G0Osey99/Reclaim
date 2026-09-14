//! QEMU QCOW2 images (`.qcow2`, v2/v3) — docs/plan/04 §5.
//!
//! Big-endian header, then a two-level cluster map: an L1 table of L2-table
//! offsets, each L2 table mapping guest clusters to host offsets. Clusters may
//! be unallocated (zeros / backing file — we treat as zeros), plain, or
//! compressed (raw DEFLATE). We flatten the whole map into a
//! [`MappedSource`](super::map::MappedSource) at open time.

use super::codec::Codec;
use super::map::{Piece, SegmentMap};
use super::{be_u32, be_u64, read_exact_vec};
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::{BlockSource, SourceId};
use std::path::Path;
use std::sync::Arc;

const MAGIC: u32 = 0x5146_49FB; // "QFI\xfb"
const OFLAG_COMPRESSED: u64 = 1 << 62;
const OFLAG_ZERO: u64 = 1; // standard cluster with bit 0 set ⇒ read as zeros
const L2_OFFSET_MASK: u64 = 0x00FF_FFFF_FFFF_FE00;
const MAX_CLUSTERS: u64 = 64 * 1024 * 1024; // DoS guard on flattened map

pub(crate) fn open(path: &Path) -> Result<super::map::MappedSource, BlockError> {
    let img = ImageFile::open(path)?;
    let backing: Arc<dyn BlockSource> = Arc::new(img);

    let hdr = read_exact_vec(&backing, 0, 104)
        .ok_or_else(|| BlockError::Container("qcow2: short header".into()))?;
    if be_u32(&hdr, 0) != Some(MAGIC) {
        return Err(BlockError::Container("qcow2: bad magic".into()));
    }
    let version = be_u32(&hdr, 4).unwrap_or(0);
    if version < 2 {
        return Err(BlockError::Container("qcow2: unsupported version".into()));
    }
    let cluster_bits =
        be_u32(&hdr, 20).ok_or_else(|| BlockError::Container("qcow2: no cluster_bits".into()))?;
    // Spec: cluster_bits is 9..=21 (512 B .. 2 MiB).
    if !(9..=21).contains(&cluster_bits) {
        return Err(BlockError::Container(
            "qcow2: implausible cluster_bits".into(),
        ));
    }
    let cluster_size = 1u64 << cluster_bits;
    let size = be_u64(&hdr, 24).ok_or_else(|| BlockError::Container("qcow2: no size".into()))?;
    let l1_size =
        be_u32(&hdr, 36).ok_or_else(|| BlockError::Container("qcow2: no l1_size".into()))?;
    let l1_offset =
        be_u64(&hdr, 40).ok_or_else(|| BlockError::Container("qcow2: no l1_offset".into()))?;
    if be_u32(&hdr, 32).unwrap_or(0) != 0 {
        return Err(BlockError::Container(
            "qcow2: encrypted (unsupported)".into(),
        ));
    }

    let l2_entries = cluster_size / 8; // u64 entries per L2 table
    let total_clusters = size.div_ceil(cluster_size);
    if total_clusters > MAX_CLUSTERS {
        return Err(BlockError::Container(
            "qcow2: image too large to map".into(),
        ));
    }

    // Read the L1 table.
    let l1_bytes = usize::try_from(u64::from(l1_size).saturating_mul(8))
        .map_err(|_| BlockError::Container("qcow2: L1 too large".into()))?;
    let l1 = read_exact_vec(&backing, l1_offset, l1_bytes)
        .ok_or_else(|| BlockError::Container("qcow2: cannot read L1".into()))?;

    // csize encoding for compressed clusters (per QCOW2 spec).
    let csize_shift = 62 - (cluster_bits - 8);
    let csize_mask = (1u64 << (cluster_bits - 8)) - 1;
    let coffset_mask = (1u64 << csize_shift) - 1;

    let mut smap = SegmentMap::new();
    // The walk is sequential, so a single current-L2-table cache suffices
    // (an unbounded per-offset cache would hold every L2 table of the image).
    let mut cur_l2: Option<(u64, Vec<u8>)> = None;

    for c in 0..total_clusters {
        let l1_index = c / l2_entries;
        let l2_index = c % l2_entries;
        let this_out = core::cmp::min(cluster_size, size.saturating_sub(c * cluster_size));
        if this_out == 0 {
            break;
        }

        let l1e = le_or_be_u64(&l1, usize::try_from(l1_index * 8).unwrap_or(usize::MAX));
        let l2_off = l1e.map_or(0, |v| v & L2_OFFSET_MASK);
        if l2_off == 0 {
            smap.push(this_out, Piece::Zero); // no L2 ⇒ unallocated
            continue;
        }

        if !matches!(&cur_l2, Some((off, _)) if *off == l2_off) {
            let t = read_exact_vec(&backing, l2_off, cluster_size as usize)
                .ok_or_else(|| BlockError::Container("qcow2: cannot read L2".into()))?;
            cur_l2 = Some((l2_off, t));
        }
        let Some((_, l2)) = &cur_l2 else {
            return Err(BlockError::Container("qcow2: cannot read L2".into()));
        };
        let l2e = be_u64(l2, usize::try_from(l2_index * 8).unwrap_or(usize::MAX)).unwrap_or(0);

        if l2e == 0 {
            smap.push(this_out, Piece::Zero);
        } else if l2e & OFLAG_COMPRESSED != 0 {
            let coffset = l2e & coffset_mask;
            let nb_csectors = ((l2e >> csize_shift) & csize_mask) + 1;
            // Compressed data spans from coffset for nb_csectors 512-byte
            // sectors (may straddle sector boundaries).
            let clen = nb_csectors.saturating_mul(512);
            smap.push(
                this_out,
                Piece::Compressed {
                    codec: Codec::Deflate,
                    pos: coffset,
                    clen,
                },
            );
        } else {
            let host = l2e & L2_OFFSET_MASK;
            if host == 0 || l2e & OFLAG_ZERO != 0 {
                smap.push(this_out, Piece::Zero);
            } else {
                smap.push(this_out, Piece::Raw { pos: host });
            }
        }
    }
    if size > smap.out_len() {
        smap.push(size - smap.out_len(), Piece::Zero);
    }
    let id = SourceId::new(format!("qcow2:{}:{}", path.display(), size));
    Ok(smap.build(backing, size, 512, id))
}

/// L1 entries are big-endian; helper kept separate for clarity/testing.
fn le_or_be_u64(b: &[u8], off: usize) -> Option<u64> {
    be_u64(b, off)
}
