//! [`OffsetView`] — a byte window into a parent [`BlockSource`], used to present
//! a partition/volume as its own source (docs/plan/03 §2.1).

use crate::error::BlockError;
use crate::source::{align_down, BlockSource, ReadResult, SourceId};
use std::sync::Arc;

/// A `[start, start+len)` window over a parent source.
pub struct OffsetView {
    inner: Arc<dyn BlockSource>,
    start: u64,
    len: u64,
    id: SourceId,
}

impl std::fmt::Debug for OffsetView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OffsetView")
            .field("start", &self.start)
            .field("len", &self.len)
            .field("id", &self.id)
            .finish()
    }
}

impl OffsetView {
    /// Create a window `[start, start+len)` over `inner`.
    ///
    /// `start` must be a multiple of the parent's sector size so LBA reporting
    /// stays coherent, and the window must fit inside the parent.
    pub fn new(inner: Arc<dyn BlockSource>, start: u64, len: u64) -> Result<Self, BlockError> {
        let ss = u64::from(inner.sector_size());
        if ss != 0 && !start.is_multiple_of(ss) {
            return Err(BlockError::Geometry(format!(
                "offset-view start {start} not aligned to sector size {ss}"
            )));
        }
        let parent_len = inner.len();
        if start > parent_len || len > parent_len - start {
            return Err(BlockError::WindowOutOfRange {
                start,
                len,
                source_len: parent_len,
            });
        }
        let id = SourceId::new(format!("view:{}:{}+{}", inner.id(), start, len));
        Ok(OffsetView {
            inner,
            start,
            len,
            id,
        })
    }

    /// The window's start offset within the parent.
    #[must_use]
    pub fn start(&self) -> u64 {
        self.start
    }
}

impl BlockSource for OffsetView {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }

    fn physical_sector_size(&self) -> u32 {
        self.inner.physical_sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult {
        let ss = u64::from(self.sector_size().max(1));
        let sector_count = buf.len().div_ceil(self.sector_size().max(1) as usize);
        // Requests beyond the window are all-bad; the parent would otherwise read
        // a neighbouring partition's bytes.
        if offset >= self.len || !offset.is_multiple_of(ss) {
            buf.fill(0);
            let mut res = ReadResult::good(self.sector_size(), sector_count);
            for i in 0..sector_count {
                res.mark_bad(i);
            }
            return res;
        }
        // Clamp reads that run past the window end by delegating only the bytes
        // inside it; the parent marks any beyond-window tail as bad.
        let parent_off = align_down(self.start, ss) + offset;
        self.inner.read_at(parent_off, buf)
    }

    fn id(&self) -> SourceId {
        self.id.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::image_file::ImageFile;
    use std::io::Write;

    fn temp(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn window_reads_translate() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let tmp = temp(&data);
        let img: Arc<dyn BlockSource> = Arc::new(ImageFile::open(tmp.path()).unwrap());
        let view = OffsetView::new(img, 1024, 2048).unwrap();
        assert_eq!(view.len(), 2048);

        let mut buf = vec![0u8; 512];
        let r = view.read_at(512, &mut buf); // parent offset 1024+512 = 1536
        assert!(r.all_good());
        assert_eq!(&buf[..], &data[1536..2048]);
    }

    #[test]
    fn rejects_unaligned_or_oversized_window() {
        let tmp = temp(&vec![0u8; 4096]);
        let img: Arc<dyn BlockSource> = Arc::new(ImageFile::open(tmp.path()).unwrap());
        assert!(OffsetView::new(img.clone(), 100, 128).is_err()); // unaligned start
        assert!(OffsetView::new(img, 2048, 4096).is_err()); // runs past parent
    }
}
