//! [`MappedImage`] — a read-only [`BlockSource`] over an image file plus its
//! `.reclaim-map` sidecar (docs/plan/03 §2.1). Never-read regions return
//! [`SectorStatus::Unread`] and known-bad regions [`SectorStatus::Bad`]; for
//! zstd-framed images, reads are served by decompressing the covering frames.

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
}

impl MappedImage {
    /// Open `image_path` with its `map`.
    pub fn open(image_path: &Path, map: ImageMap) -> std::io::Result<Self> {
        let file = File::open(image_path)?;
        crate::open_log::record_open(image_path, libc::O_RDONLY);
        let mut frames = map.frames.clone();
        frames.sort_by_key(|f| f.image_offset);
        let id = SourceId::new(format!("mapped:{}", map.source_id));
        Ok(MappedImage {
            file,
            map: Arc::new(map),
            id,
            frames,
        })
    }

    fn overlay_status(&self, offset: u64, res: &mut ReadResult) {
        let ss = u64::from(self.map.sector_size.max(1));
        for i in 0..res.sector_count() {
            let lba = (offset / ss).saturating_add(i as u64);
            if self.map.bad.iter().any(|r| r.contains(lba)) {
                res.mark_bad(i);
            } else if self.map.unread.iter().any(|r| r.contains(lba)) {
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
        let mi = MappedImage::open(&img, map).unwrap();
        let mut buf = vec![0u8; 2048];
        let r = mi.read_at(0, &mut buf);
        assert_eq!(r.status(0), SectorStatus::Good);
        assert_eq!(r.status(1), SectorStatus::Bad);
        assert_eq!(r.status(3), SectorStatus::Unread);
    }
}
