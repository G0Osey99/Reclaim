//! VirtualBox VDI images (`.vdi`) — docs/plan/04 §5.
//!
//! A text banner, then a little-endian header (signature `0xBEDA107F` at 0x40)
//! giving the block size, block count, the offset of the block-allocation table
//! and of the data area. The BAT maps each virtual block to a data-area block
//! index; the sentinels `0xFFFFFFFF` (unallocated) and `0xFFFFFFFE` (allocated
//! but zeroed) read back as zeros.

use super::map::{Piece, SegmentMap};
use super::{le_u32, le_u64, read_exact_vec};
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::{BlockSource, SourceId};
use std::path::Path;
use std::sync::Arc;

const SIG: u32 = 0xBEDA_107F;
const VDI_UNALLOCATED: u32 = 0xFFFF_FFFF;
const VDI_ZERO: u32 = 0xFFFF_FFFE;
const MAX_BLOCKS: u64 = 64 * 1024 * 1024; // BAT entry cap (DoS guard)

pub(crate) fn open(path: &Path) -> Result<super::map::MappedSource, BlockError> {
    let img = ImageFile::open(path)?;
    let backing: Arc<dyn BlockSource> = Arc::new(img);

    let hdr = read_exact_vec(&backing, 0, 0x200)
        .ok_or_else(|| BlockError::Container("vdi: short header".into()))?;
    if le_u32(&hdr, 0x40) != Some(SIG) {
        return Err(BlockError::Container("vdi: bad signature".into()));
    }
    let offset_blocks = u64::from(
        le_u32(&hdr, 0x154).ok_or_else(|| BlockError::Container("vdi: no offsetBlocks".into()))?,
    );
    let offset_data = u64::from(
        le_u32(&hdr, 0x158).ok_or_else(|| BlockError::Container("vdi: no offsetData".into()))?,
    );
    let disk_size =
        le_u64(&hdr, 0x170).ok_or_else(|| BlockError::Container("vdi: no diskSize".into()))?;
    let block_size = u64::from(
        le_u32(&hdr, 0x178).ok_or_else(|| BlockError::Container("vdi: no blockSize".into()))?,
    );
    let blocks = u64::from(
        le_u32(&hdr, 0x180).ok_or_else(|| BlockError::Container("vdi: no blocksInImage".into()))?,
    );
    if block_size == 0 || blocks == 0 || blocks > MAX_BLOCKS {
        return Err(BlockError::Container("vdi: implausible geometry".into()));
    }

    // Read the whole BAT (blocks × u32).
    let bat_bytes = usize::try_from(blocks.saturating_mul(4))
        .map_err(|_| BlockError::Container("vdi: BAT too large".into()))?;
    let bat = read_exact_vec(&backing, offset_blocks, bat_bytes)
        .ok_or_else(|| BlockError::Container("vdi: cannot read BAT".into()))?;

    let mut smap = SegmentMap::new();
    for i in 0..blocks {
        let idx = usize::try_from(i.saturating_mul(4)).unwrap_or(usize::MAX);
        let entry = le_u32(&bat, idx).unwrap_or(VDI_UNALLOCATED);
        // Clamp the final block to the declared disk size.
        let this = if (i + 1).saturating_mul(block_size) > disk_size {
            disk_size.saturating_sub(i.saturating_mul(block_size))
        } else {
            block_size
        };
        if this == 0 {
            break;
        }
        if entry == VDI_UNALLOCATED || entry == VDI_ZERO {
            smap.push(this, Piece::Zero);
        } else {
            let pos = offset_data.saturating_add(u64::from(entry).saturating_mul(block_size));
            smap.push(this, Piece::Raw { pos });
        }
    }
    if disk_size > smap.out_len() {
        smap.push(disk_size - smap.out_len(), Piece::Zero);
    }
    let id = SourceId::new(format!("vdi:{}:{}", path.display(), disk_size));
    Ok(smap.build(backing, disk_size, 512, id))
}
