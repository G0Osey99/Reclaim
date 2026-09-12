//! Apple `.sparseimage` (UDSP) — docs/plan/04 §5.
//!
//! Layout verified empirically against `hdiutil`-produced images:
//! a 4096-byte header (`sprs`, version 3, big-endian) giving `sectorsPerBand`
//! (0x08) and the total sector count (0x10), then a band index table of
//! big-endian `u32` starting at 0x40 — one entry per band. Entry `0` ⇒ the band
//! is absent (reads as zeros); entry `N` ⇒ the band's data is the `N`-th slot in
//! the data area, which begins at 0x1000 (band data offset =
//! `0x1000 + (N-1)*bandBytes`).
//!
//! Single-header band table only (≤ 1008 bands for the file's band size); very
//! large images that spill the table are reported as unsupported.

use super::map::{Piece, SegmentMap};
use super::{be_u32, read_exact_vec};
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::{BlockSource, SourceId};
use std::path::Path;
use std::sync::Arc;

const HEADER: u64 = 0x1000;
const TABLE_OFF: usize = 0x40;
const SECTOR: u64 = 512;

pub(crate) fn open(path: &Path) -> Result<super::map::MappedSource, BlockError> {
    let img = ImageFile::open(path)?;
    let backing: Arc<dyn BlockSource> = Arc::new(img);
    let hdr = read_exact_vec(&backing, 0, HEADER as usize)
        .ok_or_else(|| BlockError::Container("sparseimage: short header".into()))?;
    if hdr.get(0..4) != Some(b"sprs") {
        return Err(BlockError::Container("sparseimage: bad magic".into()));
    }
    let sectors_per_band = u64::from(
        be_u32(&hdr, 0x08)
            .ok_or_else(|| BlockError::Container("sparseimage: no sectorsPerBand".into()))?,
    );
    let total_sectors = u64::from(
        be_u32(&hdr, 0x10)
            .ok_or_else(|| BlockError::Container("sparseimage: no totalSectors".into()))?,
    );
    if sectors_per_band == 0 || total_sectors == 0 {
        return Err(BlockError::Container("sparseimage: zero geometry".into()));
    }
    let band_bytes = sectors_per_band.saturating_mul(SECTOR);
    let total_bytes = total_sectors.saturating_mul(SECTOR);
    let num_bands = total_sectors.div_ceil(sectors_per_band);

    // The band table must fit in the header.
    let table_bytes = num_bands.saturating_mul(4);
    if TABLE_OFF as u64 + table_bytes > HEADER {
        return Err(BlockError::Container(
            "sparseimage: multi-node band table unsupported".into(),
        ));
    }

    let mut smap = SegmentMap::new();
    for i in 0..num_bands {
        let this = core::cmp::min(band_bytes, total_bytes.saturating_sub(i * band_bytes));
        if this == 0 {
            break;
        }
        let entry = be_u32(
            &hdr,
            TABLE_OFF + usize::try_from(i).unwrap_or(usize::MAX) * 4,
        )
        .unwrap_or(0);
        if entry == 0 {
            smap.push(this, Piece::Zero);
        } else {
            let pos = HEADER.saturating_add((u64::from(entry) - 1).saturating_mul(band_bytes));
            smap.push(this, Piece::Raw { pos });
        }
    }
    if total_bytes > smap.out_len() {
        smap.push(total_bytes - smap.out_len(), Piece::Zero);
    }
    let id = SourceId::new(format!("sparseimage:{}:{}", path.display(), total_bytes));
    Ok(smap.build(backing, total_bytes, SECTOR as u32, id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn synthesized_sparseimage() {
        // 4 bands of 512 bytes each (sectorsPerBand=1); bands 0 and 2 present.
        let band = 512usize;
        let total_sectors = 4u32;
        let mut hdr = vec![0u8; HEADER as usize];
        hdr[0..4].copy_from_slice(b"sprs");
        hdr[4..8].copy_from_slice(&3u32.to_be_bytes());
        hdr[8..12].copy_from_slice(&1u32.to_be_bytes()); // sectors per band
        hdr[0x10..0x14].copy_from_slice(&total_sectors.to_be_bytes());
        // band table: band0→slot1, band1→absent, band2→slot2, band3→absent
        hdr[0x40..0x44].copy_from_slice(&1u32.to_be_bytes());
        hdr[0x44..0x48].copy_from_slice(&0u32.to_be_bytes());
        hdr[0x48..0x4C].copy_from_slice(&2u32.to_be_bytes());
        hdr[0x4C..0x50].copy_from_slice(&0u32.to_be_bytes());

        let mut buf = hdr;
        buf.extend_from_slice(&vec![0xAAu8; band]); // slot 1 = band 0
        buf.extend_from_slice(&vec![0xCCu8; band]); // slot 2 = band 2

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.sparseimage");
        std::fs::File::create(&p).unwrap().write_all(&buf).unwrap();

        let src = open(&p).unwrap();
        assert_eq!(src.len(), 4 * 512);
        let mut out = vec![0x55u8; 4 * 512];
        assert!(src.read_at(0, &mut out).all_good());
        assert!(out[0..512].iter().all(|b| *b == 0xAA)); // band 0
        assert!(out[512..1024].iter().all(|b| *b == 0)); // band 1 absent
        assert!(out[1024..1536].iter().all(|b| *b == 0xCC)); // band 2
        assert!(out[1536..2048].iter().all(|b| *b == 0)); // band 3 absent
    }
}
