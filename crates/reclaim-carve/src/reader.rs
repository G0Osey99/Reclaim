//! Random-access byte readers for validators.
//!
//! Validators walk a file's structure by reading arbitrary `[offset, len)`
//! ranges. [`SourceReader`] adapts a sector-aligned [`reclaim_block::BlockSource`]
//! (typically wrapped in a `ReadAheadCache`) into byte-granular reads, so the
//! carver never holds a whole file in memory (NFR-3). [`MemReader`] serves the
//! same trait over an in-memory buffer for unit tests and fuzzing.

use reclaim_block::BlockSource;
use std::sync::Arc;

/// Byte-granular, best-effort random reads. Bytes past EOF or over bad/unread
/// sectors are zero-filled; [`Reader::read_flags`] reports whether a range
/// touched any such sector so callers can mark results `Suspect`.
pub trait Reader: std::fmt::Debug {
    /// Read `[offset, offset+len)`, zero-filling anything unavailable.
    fn read(&self, offset: u64, len: usize) -> Vec<u8>;

    /// Like [`Reader::read`] but also returns `true` if any byte in the range
    /// came from a bad or never-read sector.
    fn read_flags(&self, offset: u64, len: usize) -> (Vec<u8>, bool);

    /// Total length of the underlying source.
    fn len(&self) -> u64;

    /// True if the source is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A [`Reader`] over a block source with sector-alignment handling.
pub struct SourceReader {
    src: Arc<dyn BlockSource>,
    sector_size: u64,
    len: u64,
}

impl std::fmt::Debug for SourceReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceReader")
            .field("sector_size", &self.sector_size)
            .field("len", &self.len)
            .finish()
    }
}

impl SourceReader {
    /// Wrap a block source.
    #[must_use]
    pub fn new(src: Arc<dyn BlockSource>) -> Self {
        let sector_size = u64::from(src.sector_size().max(1));
        let len = src.len();
        SourceReader {
            src,
            sector_size,
            len,
        }
    }

    fn read_inner(&self, offset: u64, len: usize, want_flags: bool) -> (Vec<u8>, bool) {
        let mut out = vec![0u8; len];
        if len == 0 || offset >= self.len {
            return (out, want_flags && offset < self.len);
        }
        let ss = self.sector_size;
        let aligned_start = offset - (offset % ss);
        let end = offset.saturating_add(len as u64);
        // Round the end up to a sector boundary.
        let aligned_end = end.div_ceil(ss).saturating_mul(ss);
        let span = (aligned_end - aligned_start) as usize;
        let mut buf = vec![0u8; span];
        let rr = self.src.read_at(aligned_start, &mut buf);
        let skip = (offset - aligned_start) as usize;
        let avail = span.saturating_sub(skip);
        let copy = len.min(avail);
        if let (Some(dst), Some(src)) = (out.get_mut(..copy), buf.get(skip..skip + copy)) {
            dst.copy_from_slice(src);
        }
        let mut suspect = false;
        if want_flags && (rr.any_bad() || rr.any_unread()) {
            // Any bad/unread sector overlapping the requested window taints it.
            let first = skip / ss as usize;
            let last = (skip + copy).saturating_sub(1) / ss as usize;
            for i in first..=last.min(rr.sector_count().saturating_sub(1)) {
                if rr.status(i) != reclaim_block::SectorStatus::Good {
                    suspect = true;
                    break;
                }
            }
        }
        (out, suspect)
    }
}

impl Reader for SourceReader {
    fn read(&self, offset: u64, len: usize) -> Vec<u8> {
        self.read_inner(offset, len, false).0
    }
    fn read_flags(&self, offset: u64, len: usize) -> (Vec<u8>, bool) {
        self.read_inner(offset, len, true)
    }
    fn len(&self) -> u64 {
        self.len
    }
}

/// A [`Reader`] over an in-memory buffer (tests, fuzzing).
#[derive(Debug, Clone)]
pub struct MemReader<'a> {
    data: &'a [u8],
}

impl<'a> MemReader<'a> {
    /// Wrap a byte buffer.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        MemReader { data }
    }
}

impl Reader for MemReader<'_> {
    fn read(&self, offset: u64, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        if let Ok(start) = usize::try_from(offset) {
            let end = start.saturating_add(len).min(self.data.len());
            if start < self.data.len() {
                if let (Some(dst), Some(src)) =
                    (out.get_mut(..end - start), self.data.get(start..end))
                {
                    dst.copy_from_slice(src);
                }
            }
        }
        out
    }
    fn read_flags(&self, offset: u64, len: usize) -> (Vec<u8>, bool) {
        (self.read(offset, len), false)
    }
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use reclaim_block::ImageFile;
    use std::io::Write;

    #[test]
    fn mem_reader_bounds() {
        let data: Vec<u8> = (0..100u8).collect();
        let r = MemReader::new(&data);
        assert_eq!(r.read(10, 4), vec![10, 11, 12, 13]);
        // Past EOF is zero-filled.
        assert_eq!(r.read(98, 4), vec![98, 99, 0, 0]);
        assert_eq!(r.len(), 100);
    }

    #[test]
    fn source_reader_unaligned() {
        let data: Vec<u8> = (0..2048u32).map(|i| (i % 256) as u8).collect();
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&data).unwrap();
        f.flush().unwrap();
        let img = ImageFile::open(f.path()).unwrap();
        let r = SourceReader::new(Arc::new(img));
        // Unaligned offset 700, length crossing sector boundaries.
        let got = r.read(700, 100);
        assert_eq!(got, data[700..800].to_vec());
        let (got2, suspect) = r.read_flags(1000, 50);
        assert_eq!(got2, data[1000..1050].to_vec());
        assert!(!suspect);
    }
}
