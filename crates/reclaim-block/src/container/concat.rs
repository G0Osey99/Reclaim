//! [`ConcatSource`] — several [`BlockSource`]s presented end-to-end as one raw
//! image. Used by split raw sets (`.001`, `.002`, …), EnCase multi-segment
//! evidence files, and multi-extent flat VMDKs.

use crate::source::{BlockSource, ReadResult, SectorStatus, SourceId};
use std::sync::Arc;

/// Concatenation of parts. Each part's length must be a multiple of the shared
/// sector size so part boundaries stay sector-aligned.
pub struct ConcatSource {
    parts: Vec<Part>,
    total: u64,
    sector_size: u32,
    id: SourceId,
}

struct Part {
    src: Arc<dyn BlockSource>,
    start: u64, // cumulative output offset where this part begins
    len: u64,
}

impl std::fmt::Debug for ConcatSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConcatSource")
            .field("parts", &self.parts.len())
            .field("total", &self.total)
            .field("id", &self.id)
            .finish()
    }
}

impl ConcatSource {
    /// Build from ordered parts. The sector size is taken from the first part
    /// (or 512 if none); parts whose length is not a multiple of it are clamped
    /// down so boundaries stay aligned.
    #[must_use]
    pub fn new(parts: Vec<Arc<dyn BlockSource>>, id: SourceId) -> Self {
        let sector_size = parts.first().map_or(512, |p| p.sector_size().max(1));
        let ss = u64::from(sector_size);
        let mut list = Vec::with_capacity(parts.len());
        let mut cursor = 0u64;
        for p in parts {
            let plen = p.len() - (p.len() % ss);
            if plen == 0 {
                continue;
            }
            list.push(Part {
                src: p,
                start: cursor,
                len: plen,
            });
            cursor = cursor.saturating_add(plen);
        }
        ConcatSource {
            parts: list,
            total: cursor,
            sector_size,
            id,
        }
    }
}

impl BlockSource for ConcatSource {
    fn len(&self) -> u64 {
        self.total
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn physical_sector_size(&self) -> u32 {
        self.sector_size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult {
        let ss = u64::from(self.sector_size);
        let sector_count = buf.len().div_ceil(self.sector_size as usize);
        let mut res = ReadResult::good(self.sector_size, sector_count);
        buf.fill(0);
        if buf.is_empty() {
            return res;
        }
        if !offset.is_multiple_of(ss) || offset >= self.total {
            for i in 0..sector_count {
                res.mark_bad(i);
            }
            return res;
        }
        let total = buf.len();
        let mut done = 0usize;
        while done < total {
            let cur = offset.saturating_add(done as u64);
            if cur >= self.total {
                mark_bad_range(&mut res, self.sector_size, done, total - done);
                break;
            }
            let Some(part) = self
                .parts
                .iter()
                .find(|p| cur >= p.start && cur < p.start + p.len)
            else {
                mark_bad_range(&mut res, self.sector_size, done, total - done);
                break;
            };
            let within = cur - part.start;
            let want = core::cmp::min(part.len - within, (total - done) as u64) as usize;
            let Some(dst) = buf.get_mut(done..done + want) else {
                break;
            };
            let r = part.src.read_at(within, dst);
            // Propagate per-sector status from the part into our result.
            if !r.all_good() {
                for i in 0..r.sector_count() {
                    if r.status(i) != SectorStatus::Good {
                        let byte = done + i * self.sector_size as usize;
                        mark_bad_range(&mut res, self.sector_size, byte, self.sector_size as usize);
                    }
                }
            }
            done += want;
        }
        res
    }

    fn id(&self) -> SourceId {
        self.id.clone()
    }
}

fn mark_bad_range(res: &mut ReadResult, sector_size: u32, start: usize, len: usize) {
    if len == 0 {
        return;
    }
    let ss = sector_size as usize;
    let first = start / ss;
    let last = start.saturating_add(len).saturating_sub(1) / ss;
    for i in first..=last {
        if i < res.sector_count() {
            res.mark_bad(i);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::memory::MemorySource;

    #[test]
    fn concat_reads_across_boundary() {
        let a: Arc<dyn BlockSource> = Arc::new(MemorySource::new(vec![0xAAu8; 1024]));
        let b: Arc<dyn BlockSource> = Arc::new(MemorySource::new(vec![0xBBu8; 1024]));
        let c = ConcatSource::new(vec![a, b], SourceId::new("cat"));
        assert_eq!(c.len(), 2048);
        let mut buf = vec![0u8; 1024];
        let r = c.read_at(512, &mut buf); // spans part A tail + part B head
        assert!(r.all_good());
        assert!(buf[..512].iter().all(|x| *x == 0xAA));
        assert!(buf[512..].iter().all(|x| *x == 0xBB));
    }
}
