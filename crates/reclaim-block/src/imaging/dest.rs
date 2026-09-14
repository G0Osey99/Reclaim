//! Image destination writer — **the only source-adjacent write path** (build
//! guide Part 3.2; allow-listed in `scripts/readonly-allowlist.txt`).
//!
//! Writes either a raw (optionally sparse) image or a zstd-framed image with a
//! random-access frame index. Chunks are written in logical order so the frame
//! index and sparse holes stay coherent across resume.

use crate::imaging::map::{Compression, FrameEntry};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

/// zstd compression level for framed output.
const ZSTD_LEVEL: i32 = 3;

/// The image output file.
#[derive(Debug)]
pub struct ImageDest {
    file: File,
    compression: Compression,
    sparse: bool,
    append_cursor: u64,
    frames: Vec<FrameEntry>,
}

/// Refuse to open an image destination that exists and is not a regular file
/// (device node, directory, FIFO, or a symlink to any of those). This is the
/// last line of defence against pointing the only writer at a source device.
fn refuse_non_regular(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_file() => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "refusing image destination {}: not a regular file",
                path.display()
            ),
        )),
        _ => Ok(()),
    }
}

impl ImageDest {
    /// Create (truncating) a new destination.
    pub fn create(path: &Path, compression: Compression, sparse: bool) -> std::io::Result<Self> {
        refuse_non_regular(path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        Ok(ImageDest {
            file,
            compression,
            sparse,
            append_cursor: 0,
            frames: Vec::new(),
        })
    }

    /// Open an existing destination for resume (no truncation). `frames` seeds
    /// the frame index and the append cursor for zstd output.
    pub fn open_append(
        path: &Path,
        compression: Compression,
        sparse: bool,
        frames: Vec<FrameEntry>,
    ) -> std::io::Result<Self> {
        refuse_non_regular(path)?;
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let append_cursor = match compression {
            Compression::Zstd => frames
                .iter()
                .map(|f| f.file_offset + u64::from(f.compressed))
                .max()
                .unwrap_or(0),
            Compression::None => 0,
        };
        Ok(ImageDest {
            file,
            compression,
            sparse,
            append_cursor,
            frames,
        })
    }

    /// The current frame index (for the sidecar map).
    #[must_use]
    pub fn frames(&self) -> &[FrameEntry] {
        &self.frames
    }

    /// Write one logical chunk at `image_offset`.
    pub fn write_chunk(&mut self, image_offset: u64, data: &[u8]) -> std::io::Result<()> {
        match self.compression {
            Compression::None => {
                // Sparse: skip all-zero chunks, leaving a hole.
                if self.sparse && data.iter().all(|b| *b == 0) {
                    return Ok(());
                }
                self.file.write_all_at(data, image_offset)
            }
            Compression::Zstd => {
                let compressed = zstd::bulk::compress(data, ZSTD_LEVEL)?;
                let file_offset = self.append_cursor;
                self.file.seek(SeekFrom::Start(file_offset))?;
                self.file.write_all(&compressed)?;
                self.append_cursor += compressed.len() as u64;
                self.frames.push(FrameEntry {
                    image_offset,
                    file_offset,
                    uncompressed: u32::try_from(data.len()).unwrap_or(u32::MAX),
                    compressed: u32::try_from(compressed.len()).unwrap_or(u32::MAX),
                });
                Ok(())
            }
        }
    }

    /// For raw output, set the logical file length so trailing holes/EOF are
    /// correct even when the last chunk was sparse-skipped.
    pub fn set_logical_len(&mut self, size: u64) -> std::io::Result<()> {
        if self.compression == Compression::None {
            self.file.set_len(size)?;
        }
        Ok(())
    }

    /// Flush to disk.
    pub fn sync(&mut self) -> std::io::Result<()> {
        self.file.flush()?;
        self.file.sync_all()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn raw_sparse_write() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.img");
        let mut d = ImageDest::create(&p, Compression::None, true).unwrap();
        d.write_chunk(0, &[1, 2, 3, 4]).unwrap();
        d.write_chunk(4, &[0, 0, 0, 0]).unwrap(); // sparse hole
        d.write_chunk(8, &[9, 9]).unwrap();
        d.set_logical_len(10).unwrap();
        d.sync().unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bytes.len(), 10);
        assert_eq!(&bytes[0..4], &[1, 2, 3, 4]);
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0]); // hole reads as zero
        assert_eq!(&bytes[8..10], &[9, 9]);
    }

    #[test]
    fn refuses_non_regular_destination() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a-directory");
        std::fs::create_dir(&sub).unwrap();
        let err = ImageDest::create(&sub, Compression::None, false).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        let err = ImageDest::open_append(&sub, Compression::None, false, Vec::new()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        // A regular file (or a path that does not exist yet) is fine.
        assert!(ImageDest::create(&dir.path().join("ok.img"), Compression::None, false).is_ok());
    }

    #[test]
    fn zstd_frames() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.zst");
        let mut d = ImageDest::create(&p, Compression::Zstd, false).unwrap();
        d.write_chunk(0, &[7u8; 256]).unwrap();
        d.write_chunk(256, &[8u8; 256]).unwrap();
        d.sync().unwrap();
        assert_eq!(d.frames().len(), 2);
        assert_eq!(d.frames()[0].image_offset, 0);
        assert_eq!(d.frames()[1].image_offset, 256);
        let raw = std::fs::read(&p).unwrap();
        let first = &raw[d.frames()[0].file_offset as usize
            ..(d.frames()[0].file_offset + u64::from(d.frames()[0].compressed)) as usize];
        assert_eq!(zstd::bulk::decompress(first, 4096).unwrap(), vec![7u8; 256]);
    }
}
