//! [`MappedImage`] — a read-only [`BlockSource`] over an image file plus its
//! `.reclaim-map` sidecar (docs/plan/03 §2.1). Never-read regions return
//! [`SectorStatus::Unread`] and known-bad regions [`SectorStatus::Bad`]; for
//! zstd-framed images, reads are served by decompressing the covering frames.

use crate::bad_block::LbaRange;
use crate::imaging::map::{Compression, FrameEntry, ImageMap};
use crate::source::{BlockSource, ReadResult, SourceId};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

/// An image + map exposed as a block source.
#[derive(Debug)]
pub struct MappedImage {
    file: File,
    map: Arc<ImageMap>,
    id: SourceId,
    /// Frames sorted by image_offset (zstd only).
    frames: Vec<FrameEntry>,
    /// `map.bad` / `map.unread`, sorted and coalesced for binary search.
    bad: Vec<LbaRange>,
    unread: Vec<LbaRange>,
}

/// Binary search over sorted, coalesced ranges.
fn in_ranges(ranges: &[LbaRange], lba: u64) -> bool {
    let i = ranges.partition_point(|r| r.start <= lba);
    i > 0 && ranges.get(i - 1).is_some_and(|r| r.contains(lba))
}

impl MappedImage {
    /// Open `image_path` with its `map`.
    pub fn open(image_path: &Path, map: ImageMap) -> std::io::Result<Self> {
        let file = File::open(image_path)?;
        crate::open_log::record_open(image_path, libc::O_RDONLY);
        let mut frames = map.frames.clone();
        frames.sort_by_key(|f| f.image_offset);
        // The imager coalesces at checkpoints, but a map may have been written
        // (or edited) elsewhere: normalize once so lookups can binary-search.
        let mut bad = map.bad.clone();
        crate::imaging::imager::coalesce(&mut bad);
        let mut unread = map.unread.clone();
        crate::imaging::imager::coalesce(&mut unread);
        let id = SourceId::new(format!("mapped:{}", map.source_id));
        Ok(MappedImage {
            file,
            map: Arc::new(map),
            id,
            frames,
            bad,
            unread,
        })
    }

    fn overlay_status(&self, offset: u64, res: &mut ReadResult) {
        let ss = u64::from(self.map.sector_size.max(1));
        let cs = u64::from(self.map.chunk_size.max(1));
        for i in 0..res.sector_count() {
            let lba = (offset / ss).saturating_add(i as u64);
            let chunk = usize::try_from(lba.saturating_mul(ss) / cs).unwrap_or(usize::MAX);
            if in_ranges(&self.bad, lba) {
                res.mark_bad(i);
            } else if in_ranges(&self.unread, lba) || !self.map.is_chunk_done(chunk) {
                // A chunk that was never completed (no hash) holds no imaged
                // bytes even if the `unread` list does not mention it.
                res.mark_unread(i);
            }
        }
    }

    fn read_zstd(&self, offset: u64, buf: &mut [u8]) {
        buf.fill(0);
        let want_end = offset.saturating_add(buf.len() as u64);
        for fr in &self.frames {
            let f_start = fr.image_offset;
            let f_end = fr.image_offset + u64::from(fr.uncompressed);
            if f_end <= offset || f_start >= want_end {
                continue;
            }
            let mut comp = vec![0u8; fr.compressed as usize];
            if self.file.read_at(&mut comp, fr.file_offset).is_err() {
                continue;
            }
            let plain = match zstd::bulk::decompress(&comp, fr.uncompressed as usize) {
                Ok(p) => p,
                Err(_) => continue,
            };
            // Copy the overlapping portion into buf.
            let copy_start = offset.max(f_start);
            let copy_end = want_end.min(f_end);
            if copy_end <= copy_start {
                continue;
            }
            let src_off = (copy_start - f_start) as usize;
            let dst_off = (copy_start - offset) as usize;
            let n = (copy_end - copy_start) as usize;
            if let (Some(dst), Some(src)) = (
                buf.get_mut(dst_off..dst_off + n),
                plain.get(src_off..src_off + n),
            ) {
                dst.copy_from_slice(src);
            }
        }
    }
}

impl BlockSource for MappedImage {
    fn len(&self) -> u64 {
        self.map.size
    }
    fn sector_size(&self) -> u32 {
        self.map.sector_size.max(1)
    }
    fn physical_sector_size(&self) -> u32 {
        self.map.sector_size.max(1)
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult {
        let ss = self.map.sector_size.max(1);
        let count = buf.len().div_ceil(ss as usize);
        let mut res = ReadResult::good(ss, count);
        if !offset.is_multiple_of(u64::from(ss)) {
            for i in 0..count {
                res.mark_bad(i);
            }
            buf.fill(0);
            return res;
        }
        match self.map.compression {
            Compression::None => {
                let r = crate::image_file::read_file_at(&self.file, self.map.size, ss, offset, buf);
                // Merge the raw-read status with the map's recorded bad/unread.
                for i in r.bad_sectors() {
                    res.mark_bad(i);
                }
            }
            Compression::Zstd => {
                self.read_zstd(offset, buf);
            }
        }
        self.overlay_status(offset, &mut res);
        res
    }
    fn id(&self) -> SourceId {
        self.id.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::imaging::map::HashAlgo;
    use crate::source::SectorStatus;

    #[test]
    fn unread_and_bad_overlay() {
        use std::io::Write;
        let mut tf = tempfile::NamedTempFile::new().unwrap();
        tf.write_all(&[0xABu8; 2048]).unwrap();
        tf.flush().unwrap();
        let img = tf.path().to_path_buf();
        let mut map = ImageMap::new(
            "t".into(),
            2048,
            512,
            512,
            HashAlgo::Blake3,
            Compression::None,
        );
        map.bad
            .push(crate::bad_block::LbaRange { start: 1, count: 1 });
        map.unread
            .push(crate::bad_block::LbaRange { start: 3, count: 1 });
        for h in &mut map.chunk_hashes {
            *h = Some("done".into());
        }
        let mi = MappedImage::open(&img, map).unwrap();
        let mut buf = vec![0u8; 2048];
        let r = mi.read_at(0, &mut buf);
        assert_eq!(r.status(0), SectorStatus::Good);
        assert_eq!(r.status(1), SectorStatus::Bad);
        assert_eq!(r.status(3), SectorStatus::Unread);
    }

    #[test]
    fn incomplete_chunks_read_as_unread() {
        use std::io::Write;
        let mut tf = tempfile::NamedTempFile::new().unwrap();
        tf.write_all(&[0xCDu8; 2048]).unwrap();
        tf.flush().unwrap();
        let mut map = ImageMap::new(
            "t".into(),
            2048,
            512,
            512,
            HashAlgo::Blake3,
            Compression::None,
        );
        // Only chunk 2 was imaged; unsorted, overlapping bad ranges get coalesced.
        map.chunk_hashes[2] = Some("done".into());
        map.bad
            .push(crate::bad_block::LbaRange { start: 3, count: 1 });
        map.bad
            .push(crate::bad_block::LbaRange { start: 3, count: 1 });
        let mi = MappedImage::open(tf.path(), map).unwrap();
        assert_eq!(mi.bad.len(), 1);
        let mut buf = vec![0u8; 2048];
        let r = mi.read_at(0, &mut buf);
        assert_eq!(r.status(0), SectorStatus::Unread);
        assert_eq!(r.status(1), SectorStatus::Unread);
        assert_eq!(r.status(2), SectorStatus::Good);
        assert_eq!(r.status(3), SectorStatus::Bad);
    }

    #[test]
    fn range_lookup_is_exact_at_boundaries() {
        let rs = vec![
            LbaRange { start: 2, count: 2 },
            LbaRange {
                start: 10,
                count: 1,
            },
        ];
        assert!(!in_ranges(&rs, 1));
        assert!(in_ranges(&rs, 2));
        assert!(in_ranges(&rs, 3));
        assert!(!in_ranges(&rs, 4));
        assert!(in_ranges(&rs, 10));
        assert!(!in_ranges(&rs, 11));
        assert!(!in_ranges(&[], 0));
    }
}
