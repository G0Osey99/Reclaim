//! [`ReadAheadCache`] — an LRU cache of 1 MiB chunks over a parent source
//! (docs/plan/03 §2.1). Metadata walkers do many small random reads; caching
//! whole chunks keeps a failing drive from thrashing and amortises `pread`
//! overhead. Per-sector bad status is preserved per chunk so a cached bad
//! sector stays bad on re-read.

use crate::bitset::Bitset;
use crate::image_file::mark_bad_from_byte;
use crate::source::{align_up, BlockSource, ReadResult, SourceId};
use crate::CHUNK_SIZE;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Default number of 1 MiB chunks kept resident (64 MiB).
pub const DEFAULT_CAPACITY_CHUNKS: usize = 64;

#[derive(Debug)]
struct CacheEntry {
    data: Vec<u8>,
    bad: Bitset,
    last_used: u64,
}

#[derive(Debug, Default)]
struct CacheState {
    map: HashMap<u64, CacheEntry>,
    tick: u64,
}

/// LRU read-ahead cache wrapping an `Arc<dyn BlockSource>`.
pub struct ReadAheadCache {
    inner: Arc<dyn BlockSource>,
    chunk_size: u64,
    sector_size: u32,
    physical_sector_size: u32,
    len: u64,
    capacity_chunks: usize,
    state: Mutex<CacheState>,
    id: SourceId,
}

impl std::fmt::Debug for ReadAheadCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadAheadCache")
            .field("chunk_size", &self.chunk_size)
            .field("sector_size", &self.sector_size)
            .field("len", &self.len)
            .field("capacity_chunks", &self.capacity_chunks)
            .field("id", &self.id)
            .finish()
    }
}

impl ReadAheadCache {
    /// Wrap `inner` with the default capacity.
    #[must_use]
    pub fn new(inner: Arc<dyn BlockSource>) -> Self {
        Self::with_capacity(inner, DEFAULT_CAPACITY_CHUNKS)
    }

    /// Wrap `inner`, keeping at most `capacity_chunks` chunks resident.
    #[must_use]
    pub fn with_capacity(inner: Arc<dyn BlockSource>, capacity_chunks: usize) -> Self {
        let sector_size = inner.sector_size().max(1);
        let physical_sector_size = inner.physical_sector_size().max(sector_size);
        let len = inner.len();
        // Chunk size must be a multiple of the sector size for aligned reads.
        let chunk_size = align_up(CHUNK_SIZE, u64::from(sector_size));
        let id = SourceId::new(format!("cache:{}", inner.id()));
        ReadAheadCache {
            inner,
            chunk_size,
            sector_size,
            physical_sector_size,
            len,
            capacity_chunks: capacity_chunks.max(1),
            state: Mutex::new(CacheState::default()),
            id,
        }
    }

    /// Number of chunks currently resident (for tests/introspection).
    #[must_use]
    pub fn resident_chunks(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map
            .len()
    }

    /// Read `chunk_size` (or less, at EOF) bytes for chunk `idx` from the parent.
    fn fetch(&self, idx: u64) -> CacheEntry {
        let chunk_start = idx.saturating_mul(self.chunk_size);
        let chunk_len = core::cmp::min(self.chunk_size, self.len.saturating_sub(chunk_start));
        let mut data = vec![0u8; chunk_len as usize];
        let bad = if chunk_len == 0 {
            Bitset::new(0)
        } else {
            let rr = self.inner.read_at(chunk_start, &mut data);
            let mut b = Bitset::new(rr.sector_count());
            for s in rr.bad_sectors() {
                b.set(s);
            }
            b
        };
        CacheEntry {
            data,
            bad,
            last_used: 0,
        }
    }

    /// Evict least-recently-used entries until within capacity.
    fn evict(state: &mut CacheState, capacity: usize) {
        while state.map.len() > capacity {
            let victim = state
                .map
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| *k);
            match victim {
                Some(k) => {
                    state.map.remove(&k);
                }
                None => break,
            }
        }
    }
}

impl BlockSource for ReadAheadCache {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn physical_sector_size(&self) -> u32 {
        self.physical_sector_size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult {
        let ss = u64::from(self.sector_size);
        let ss_usize = self.sector_size as usize;
        let sector_count = buf.len().div_ceil(ss_usize);
        let mut res = ReadResult::good(self.sector_size, sector_count);
        buf.fill(0);

        if buf.is_empty() {
            return res;
        }
        if !offset.is_multiple_of(ss) || offset >= self.len {
            for i in 0..sector_count {
                res.mark_bad(i);
            }
            return res;
        }

        let end = offset + buf.len() as u64;
        let readable_end = core::cmp::min(end, self.len);

        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut pos = offset;
        while pos < readable_end {
            let idx = pos / self.chunk_size;
            let chunk_start = idx.saturating_mul(self.chunk_size);

            // Ensure the chunk is resident, then bump its recency. `fetch`
            // borrows `&self` (self.inner), distinct from the `guard.map` borrow.
            guard.tick = guard.tick.saturating_add(1);
            let tick = guard.tick;
            let entry = guard.map.entry(idx).or_insert_with(|| self.fetch(idx));
            entry.last_used = tick;
            let data_len = entry.data.len();

            let within = (pos - chunk_start) as usize;
            if within >= data_len {
                break;
            }
            let span = core::cmp::min(data_len - within, (readable_end - pos) as usize);
            let buf_off = (pos - offset) as usize;

            // Copy chunk bytes into the caller's buffer.
            if let (Some(src), Some(dst)) = (
                guard
                    .map
                    .get(&idx)
                    .and_then(|e| e.data.get(within..within + span)),
                buf.get_mut(buf_off..buf_off + span),
            ) {
                dst.copy_from_slice(src);
            }

            // Propagate per-sector bad status for the request sectors in this span.
            let first_r = buf_off / ss_usize;
            let last_r = (buf_off + span - 1) / ss_usize;
            for r in first_r..=last_r {
                let abs = offset + (r as u64) * ss;
                let chunk_rel = (abs - chunk_start) as usize;
                let csec = chunk_rel / ss_usize;
                let is_bad = guard.map.get(&idx).is_some_and(|e| e.bad.get(csec));
                if is_bad {
                    res.mark_bad(r);
                    let sstart = r * ss_usize;
                    let send = core::cmp::min(sstart + ss_usize, buf.len());
                    if let Some(sect) = buf.get_mut(sstart..send) {
                        sect.fill(0);
                    }
                }
            }

            pos += span as u64;
        }

        ReadAheadCache::evict(&mut guard, self.capacity_chunks);
        drop(guard);

        // Bytes beyond EOF (request ran past len) are bad.
        if readable_end < end {
            let first_bad = (readable_end - offset) as usize;
            mark_bad_from_byte(&mut res, self.sector_size, buf.len(), first_bad);
        }
        res
    }

    fn id(&self) -> SourceId {
        self.id.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fault::FaultInjector;
    use crate::image_file::ImageFile;
    use std::io::Write;

    fn temp(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn cache_matches_direct_reads() {
        // 3 MiB of pseudo-random-ish data spanning multiple chunks.
        let data: Vec<u8> = (0..3 * 1024 * 1024u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let tmp = temp(&data);
        let img: Arc<dyn BlockSource> = Arc::new(ImageFile::open(tmp.path()).unwrap());
        let cache = ReadAheadCache::with_capacity(img, 2);

        // A read crossing chunk boundaries (near 1 MiB).
        let off = 1024 * 1024 - 4096;
        let mut buf = vec![0u8; 8192];
        let r = cache.read_at(off, &mut buf);
        assert!(r.all_good());
        assert_eq!(&buf[..], &data[off as usize..off as usize + 8192]);

        // Re-read served from cache; identical.
        let mut buf2 = vec![0u8; 8192];
        let r2 = cache.read_at(off, &mut buf2);
        assert!(r2.all_good());
        assert_eq!(buf, buf2);
        assert!(cache.resident_chunks() <= 2, "capacity honored");
    }

    #[test]
    fn cache_preserves_bad_sectors() {
        let data = vec![0xEEu8; 2 * 1024 * 1024];
        let tmp = temp(&data);
        let img: Arc<dyn BlockSource> = Arc::new(ImageFile::open(tmp.path()).unwrap());
        // Inject a bad sector at LBA 10.
        let faulty: Arc<dyn BlockSource> = Arc::new(FaultInjector::new(img, [10u64]));
        let cache = ReadAheadCache::new(faulty);

        let mut buf = vec![0u8; 16 * 512];
        let r = cache.read_at(0, &mut buf);
        assert_eq!(r.bad_count(), 1);
        assert_eq!(r.bad_sectors().collect::<Vec<_>>(), vec![10]);
        assert!(buf[10 * 512..11 * 512].iter().all(|b| *b == 0));
        assert!(buf[0..512].iter().all(|b| *b == 0xEE));

        // Cached re-read still reports the bad sector.
        let mut buf2 = vec![0u8; 16 * 512];
        let r2 = cache.read_at(0, &mut buf2);
        assert_eq!(r2.bad_sectors().collect::<Vec<_>>(), vec![10]);
    }

    #[test]
    fn eviction_keeps_capacity() {
        let data = vec![1u8; 5 * 1024 * 1024];
        let tmp = temp(&data);
        let img: Arc<dyn BlockSource> = Arc::new(ImageFile::open(tmp.path()).unwrap());
        let cache = ReadAheadCache::with_capacity(img, 2);
        for chunk in 0..5u64 {
            let mut buf = vec![0u8; 4096];
            let _ = cache.read_at(chunk * 1024 * 1024, &mut buf);
        }
        assert!(cache.resident_chunks() <= 2);
    }
}
