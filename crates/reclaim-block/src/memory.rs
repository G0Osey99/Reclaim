//! [`MemorySource`] — an in-memory byte buffer exposed as a read-only
//! [`BlockSource`]. Used by unit tests and fuzz harnesses to drive the
//! filesystem/partition parsers over arbitrary bytes without touching disk.

use crate::source::{BlockSource, ReadResult, SourceId};
use crate::DEFAULT_SECTOR_SIZE;
use std::sync::Arc;

/// A read-only [`BlockSource`] backed by an in-memory `Vec<u8>`.
#[derive(Clone, Debug)]
pub struct MemorySource {
    data: Arc<Vec<u8>>,
    sector_size: u32,
    id: SourceId,
}

impl MemorySource {
    /// Wrap `data` with the default 512-byte sector size.
    #[must_use]
    pub fn new(data: Vec<u8>) -> Self {
        Self::with_sector_size(data, DEFAULT_SECTOR_SIZE)
    }

    /// Wrap `data` declaring a logical `sector_size` (clamped to at least 1).
    #[must_use]
    pub fn with_sector_size(data: Vec<u8>, sector_size: u32) -> Self {
        let len = data.len() as u64;
        MemorySource {
            data: Arc::new(data),
            sector_size: sector_size.max(1),
            id: SourceId::new(format!("mem:{len}")),
        }
    }
}

impl BlockSource for MemorySource {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn physical_sector_size(&self) -> u32 {
        self.sector_size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult {
        let ss = self.sector_size;
        let sector_count = buf.len().div_ceil(ss as usize);
        buf.fill(0);
        // The trait requires a sector-aligned offset; an unaligned one is all-bad.
        if !offset.is_multiple_of(u64::from(ss)) {
            let mut r = ReadResult::good(ss, sector_count);
            for i in 0..sector_count {
                r.mark_bad(i);
            }
            return r;
        }
        let mut r = ReadResult::good(ss, sector_count);
        let total = self.data.len() as u64;
        for i in 0..sector_count {
            let sect_off = offset.saturating_add((i * ss as usize) as u64);
            if sect_off >= total {
                // Past EOF: leave zero-filled and mark bad (no such sector).
                r.mark_bad(i);
                continue;
            }
            let start = sect_off as usize;
            let want = (ss as usize).min(buf.len() - i * ss as usize);
            let avail = self.data.len().saturating_sub(start);
            let copy = want.min(avail);
            if let (Some(dst), Some(src)) = (
                buf.get_mut(i * ss as usize..i * ss as usize + copy),
                self.data.get(start..start + copy),
            ) {
                dst.copy_from_slice(src);
            }
        }
        r
    }

    fn id(&self) -> SourceId {
        self.id.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn reads_within_and_past_eof() {
        let data: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let src = MemorySource::new(data.clone());
        let mut buf = vec![0u8; 512];
        let r = src.read_at(0, &mut buf);
        assert!(r.all_good());
        assert_eq!(&buf[..], &data[..512]);
        // Read spanning EOF: offset 512, 1024 bytes → sector 0 (512..1024) is a
        // valid partial, sector 1 (1024..1536) is entirely past EOF → bad.
        let mut buf2 = vec![0u8; 1024];
        let r2 = src.read_at(512, &mut buf2);
        assert_eq!(&buf2[..488], &data[512..1000]);
        assert!(r2.any_bad());
        assert_eq!(r2.status(0), crate::SectorStatus::Good);
        assert_eq!(r2.status(1), crate::SectorStatus::Bad);
    }

    #[test]
    fn unaligned_is_all_bad() {
        let src = MemorySource::new(vec![1u8; 1024]);
        let mut buf = vec![0u8; 512];
        let r = src.read_at(100, &mut buf);
        assert!(r.any_bad());
    }
}
