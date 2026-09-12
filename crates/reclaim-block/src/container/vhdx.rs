//! Microsoft VHDX images (`.vhdx`) — docs/plan/04 §5.
//!
//! `vhdxfile` identifier, then a region table pointing at a Metadata region
//! (virtual size, block size, sector size) and a BAT region. The BAT
//! interleaves payload-block entries with sector-bitmap entries; each payload
//! entry's state says whether the block is present (data at `FileOffsetMB` × 1
//! MiB) or a hole. Non-differencing dynamic/fixed images are supported; parent
//! (`HasParent`) images are not (their partially-present blocks read as zeros).

use super::map::{Piece, SegmentMap};
use super::{le_u32, le_u64, read_exact_vec};
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::{BlockSource, SourceId};
use std::path::Path;
use std::sync::Arc;

const MB: u64 = 1024 * 1024;
const MAX_BLOCKS: u64 = 16 * 1024 * 1024;

// On-disk (little-endian mixed) GUID byte patterns.
const GUID_REGION_BAT: [u8; 16] = [
    0x66, 0x77, 0xC2, 0x2D, 0x23, 0xF6, 0x00, 0x42, 0x9D, 0x64, 0x11, 0x5E, 0x9B, 0xFD, 0x4A, 0x08,
];
const GUID_REGION_META: [u8; 16] = [
    0x06, 0xA2, 0x7C, 0x8B, 0x90, 0x47, 0x9A, 0x4B, 0xB8, 0xFE, 0x57, 0x5F, 0x05, 0x0F, 0x88, 0x6E,
];
const GUID_META_FILEPARAMS: [u8; 16] = [
    0x37, 0x67, 0xA1, 0xCA, 0x36, 0xFA, 0x43, 0x4D, 0xB3, 0xB6, 0x33, 0xF0, 0xAA, 0x44, 0xE7, 0x6B,
];
const GUID_META_VDSIZE: [u8; 16] = [
    0x24, 0x42, 0xA5, 0x2F, 0x1B, 0xCD, 0x76, 0x48, 0xB2, 0x11, 0x5D, 0xBE, 0xD8, 0x3B, 0xF4, 0xB8,
];
const GUID_META_LOGSECT: [u8; 16] = [
    0x1D, 0xBF, 0x41, 0x81, 0x6F, 0xA9, 0x09, 0x47, 0xBA, 0x47, 0xF2, 0x33, 0xA8, 0xFA, 0xAB, 0x5F,
];

pub(crate) fn open(path: &Path) -> Result<super::map::MappedSource, BlockError> {
    let img = ImageFile::open(path)?;
    let backing: Arc<dyn BlockSource> = Arc::new(img);

    let ident = read_exact_vec(&backing, 0, 8)
        .ok_or_else(|| BlockError::Container("vhdx: short file".into()))?;
    if ident.get(0..8) != Some(b"vhdxfile") {
        return Err(BlockError::Container("vhdx: bad signature".into()));
    }

    // Region tables live at 0x30000 and 0x40000; use whichever parses.
    let (bat_off, meta_off) = read_region_table(&backing, 0x3_0000)
        .or_else(|| read_region_table(&backing, 0x4_0000))
        .ok_or_else(|| BlockError::Container("vhdx: no usable region table".into()))?;

    let meta = read_metadata(&backing, meta_off)?;
    let block_size = meta.block_size;
    let virtual_size = meta.virtual_size;
    let logical_sector = meta.logical_sector.max(512);
    if block_size == 0 || virtual_size == 0 {
        return Err(BlockError::Container("vhdx: zero geometry".into()));
    }

    let total_blocks = virtual_size.div_ceil(block_size);
    if total_blocks > MAX_BLOCKS {
        return Err(BlockError::Container("vhdx: too many blocks".into()));
    }
    // chunk_ratio = (2^23 * logical_sector) / block_size
    let chunk_ratio = ((1u64 << 23).saturating_mul(logical_sector)) / block_size.max(1);
    let chunk_ratio = chunk_ratio.max(1);

    let mut smap = SegmentMap::new();
    for b in 0..total_blocks {
        let this_out = core::cmp::min(block_size, virtual_size.saturating_sub(b * block_size));
        if this_out == 0 {
            break;
        }
        // BAT index accounts for interleaved sector-bitmap entries.
        let bat_index = b + (b / chunk_ratio);
        let entry_off = bat_off.saturating_add(bat_index.saturating_mul(8));
        let raw = read_exact_vec(&backing, entry_off, 8).and_then(|v| le_u64(&v, 0));
        let entry = raw.unwrap_or(0);
        let state = entry & 0x7;
        let file_offset_mb = entry >> 20; // bits 20.. = offset in MB
                                          // State 6 = PAYLOAD_BLOCK_FULLY_PRESENT.
        if state == 6 && file_offset_mb != 0 {
            smap.push(
                this_out,
                Piece::Raw {
                    pos: file_offset_mb.saturating_mul(MB),
                },
            );
        } else {
            smap.push(this_out, Piece::Zero);
        }
    }
    if virtual_size > smap.out_len() {
        smap.push(virtual_size - smap.out_len(), Piece::Zero);
    }
    let id = SourceId::new(format!("vhdx:{}:{}", path.display(), virtual_size));
    Ok(smap.build(backing, virtual_size, logical_sector as u32, id))
}

/// Returns (bat_region_offset, metadata_region_offset).
fn read_region_table(backing: &Arc<dyn BlockSource>, at: u64) -> Option<(u64, u64)> {
    let tbl = read_exact_vec(backing, at, 64 * 1024)?;
    if tbl.get(0..4) != Some(b"regi") {
        return None;
    }
    let count = le_u32(&tbl, 8)? as usize;
    if count > 2047 {
        return None;
    }
    let mut bat = None;
    let mut meta = None;
    for i in 0..count {
        let base = 16 + i * 32;
        let guid = tbl.get(base..base + 16)?;
        let file_offset = le_u64(&tbl, base + 16)?;
        if guid == GUID_REGION_BAT {
            bat = Some(file_offset);
        } else if guid == GUID_REGION_META {
            meta = Some(file_offset);
        }
    }
    Some((bat?, meta?))
}

struct VhdxMeta {
    block_size: u64,
    virtual_size: u64,
    logical_sector: u64,
}

fn read_metadata(backing: &Arc<dyn BlockSource>, region_off: u64) -> Result<VhdxMeta, BlockError> {
    // The metadata table header + entries live in the first 64 KiB; item data is
    // at an offset *relative to the region start* that may lie past 64 KiB, so
    // each item's bytes are fetched from the backing directly.
    let hdr = read_exact_vec(backing, region_off, 64 * 1024)
        .ok_or_else(|| BlockError::Container("vhdx: cannot read metadata region".into()))?;
    if hdr.get(0..8) != Some(b"metadata") {
        return Err(BlockError::Container("vhdx: bad metadata signature".into()));
    }
    let count = super::le_u16(&hdr, 10).unwrap_or(0) as usize;
    let mut block_size = 0u64;
    let mut virtual_size = 0u64;
    let mut logical_sector = 512u64;
    for i in 0..count.min(2047) {
        let base = 32 + i * 32;
        let Some(guid) = hdr.get(base..base + 16) else {
            break;
        };
        let item_off = u64::from(le_u32(&hdr, base + 16).unwrap_or(0));
        let item_len = le_u32(&hdr, base + 20).unwrap_or(0) as usize;
        let item = read_exact_vec(
            backing,
            region_off.saturating_add(item_off),
            item_len.min(4096),
        )
        .unwrap_or_default();
        if guid == GUID_META_FILEPARAMS {
            block_size = u64::from(le_u32(&item, 0).unwrap_or(0));
        } else if guid == GUID_META_VDSIZE {
            virtual_size = le_u64(&item, 0).unwrap_or(0);
        } else if guid == GUID_META_LOGSECT {
            logical_sector = u64::from(le_u32(&item, 0).unwrap_or(512));
        }
    }
    Ok(VhdxMeta {
        block_size,
        virtual_size,
        logical_sector,
    })
}
