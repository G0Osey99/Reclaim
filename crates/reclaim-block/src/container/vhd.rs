//! Microsoft VHD images (`.vhd`) — docs/plan/04 §5.
//!
//! A 512-byte big-endian `conectix` footer at the end of the file declares the
//! virtual size and disk type. **Fixed** disks are raw data followed by the
//! footer. **Dynamic** disks add a `cxsparse` header and a Block Allocation
//! Table; each allocated block carries a per-sector bitmap followed by the
//! block data.

use super::map::{Piece, SegmentMap};
use super::{be_u32, be_u64, read_exact_vec};
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::{BlockSource, SourceId};
use std::path::Path;
use std::sync::Arc;

const SECTOR: u64 = 512;
const BAT_UNUSED: u32 = 0xFFFF_FFFF;
const MAX_BLOCKS: u64 = 16 * 1024 * 1024;
const MAX_BLOCK_SIZE: u64 = 64 * 1024 * 1024;
/// VHD's CHS geometry tops out at ~2040 GiB.
const MAX_CURRENT_SIZE: u64 = 2040 * 1024 * 1024 * 1024;

pub(crate) fn open(path: &Path) -> Result<Arc<dyn BlockSource>, BlockError> {
    let img = ImageFile::open(path)?;
    let backing: Arc<dyn BlockSource> = Arc::new(img);
    let len = backing.len();
    if len < SECTOR {
        return Err(BlockError::Container("vhd: too small".into()));
    }
    let footer = read_exact_vec(&backing, len - SECTOR, SECTOR as usize)
        .ok_or_else(|| BlockError::Container("vhd: cannot read footer".into()))?;
    if footer.get(0..8) != Some(b"conectix") {
        return Err(BlockError::Container("vhd: bad footer cookie".into()));
    }
    let data_offset = be_u64(&footer, 16).unwrap_or(u64::MAX);
    let current_size =
        be_u64(&footer, 48).ok_or_else(|| BlockError::Container("vhd: no currentSize".into()))?;
    let disk_type =
        be_u32(&footer, 60).ok_or_else(|| BlockError::Container("vhd: no diskType".into()))?;

    match disk_type {
        2 => {
            // Fixed: raw payload [0, current_size); expose as one raw segment.
            let mut smap = SegmentMap::new();
            smap.push(current_size, Piece::Raw { pos: 0 });
            let id = SourceId::new(format!("vhd-fixed:{}:{}", path.display(), current_size));
            Ok(Arc::new(smap.build(
                backing,
                current_size,
                SECTOR as u32,
                id,
            )))
        }
        3 | 4 => open_dynamic(path, &backing, data_offset, current_size),
        _ => Err(BlockError::Container("vhd: unknown disk type".into())),
    }
}

fn open_dynamic(
    path: &Path,
    backing: &Arc<dyn BlockSource>,
    data_offset: u64,
    current_size: u64,
) -> Result<Arc<dyn BlockSource>, BlockError> {
    // Dynamic Disk Header ("cxsparse") at data_offset, 1024 bytes.
    let dyn_hdr = read_exact_vec(backing, data_offset, 1024)
        .ok_or_else(|| BlockError::Container("vhd: cannot read dynamic header".into()))?;
    if dyn_hdr.get(0..8) != Some(b"cxsparse") {
        return Err(BlockError::Container("vhd: bad dynamic cookie".into()));
    }
    let table_offset =
        be_u64(&dyn_hdr, 16).ok_or_else(|| BlockError::Container("vhd: no tableOffset".into()))?;
    let max_entries = u64::from(
        be_u32(&dyn_hdr, 28)
            .ok_or_else(|| BlockError::Container("vhd: no maxTableEntries".into()))?,
    );
    let block_size = u64::from(
        be_u32(&dyn_hdr, 32).ok_or_else(|| BlockError::Container("vhd: no blockSize".into()))?,
    );
    if block_size == 0
        || !block_size.is_multiple_of(SECTOR)
        || block_size > MAX_BLOCK_SIZE
        || current_size > MAX_CURRENT_SIZE
        || max_entries == 0
        || max_entries > MAX_BLOCKS
    {
        return Err(BlockError::Container("vhd: implausible geometry".into()));
    }
    let sectors_per_block = block_size / SECTOR;
    // Per-block bitmap padded up to a sector boundary.
    let bitmap_bytes = sectors_per_block.div_ceil(8);
    let bitmap_sectors = bitmap_bytes.div_ceil(SECTOR).max(1);
    let bitmap_span = bitmap_sectors.saturating_mul(SECTOR);

    // Read the BAT (max_entries × u32, big-endian).
    let bat_len = usize::try_from(max_entries.saturating_mul(4))
        .map_err(|_| BlockError::Container("vhd: BAT too large".into()))?;
    let bat = read_exact_vec(backing, table_offset, bat_len)
        .ok_or_else(|| BlockError::Container("vhd: cannot read BAT".into()))?;

    let mut smap = SegmentMap::new();
    for i in 0..max_entries {
        let this_out = core::cmp::min(block_size, current_size.saturating_sub(i * block_size));
        if this_out == 0 {
            break;
        }
        let ent = be_u32(&bat, usize::try_from(i * 4).unwrap_or(usize::MAX)).unwrap_or(BAT_UNUSED);
        if ent == BAT_UNUSED {
            smap.push(this_out, Piece::Zero);
            continue;
        }
        let block_start = u64::from(ent).saturating_mul(SECTOR);
        let data_start = block_start.saturating_add(bitmap_span);
        // Read the sector bitmap to distinguish present vs absent sectors.
        let bm = read_exact_vec(backing, block_start, bitmap_bytes as usize).unwrap_or_default();
        // Fast path: a fully-present block is one contiguous raw run.
        let needed = this_out.div_ceil(SECTOR);
        if (0..needed).all(|s| bit_set(&bm, s)) {
            smap.push(this_out, Piece::Raw { pos: data_start });
            continue;
        }
        let mut sec = 0u64;
        let mut produced = 0u64;
        while produced < this_out {
            let present = bit_set(&bm, sec);
            let piece = if present {
                Piece::Raw {
                    pos: data_start.saturating_add(sec.saturating_mul(SECTOR)),
                }
            } else {
                Piece::Zero
            };
            let out = core::cmp::min(SECTOR, this_out - produced);
            smap.push(out, piece);
            produced += out;
            sec += 1;
        }
    }
    if current_size > smap.out_len() {
        smap.push(current_size - smap.out_len(), Piece::Zero);
    }
    let id = SourceId::new(format!("vhd-dyn:{}:{}", path.display(), current_size));
    Ok(Arc::new(smap.build(
        backing.clone(),
        current_size,
        SECTOR as u32,
        id,
    )))
}

/// VHD block bitmaps are MSB-first per byte.
fn bit_set(bm: &[u8], sector: u64) -> bool {
    let byte = usize::try_from(sector / 8).unwrap_or(usize::MAX);
    let bit = 7 - (sector % 8) as u32;
    bm.get(byte).is_some_and(|b| (b >> bit) & 1 == 1)
}
