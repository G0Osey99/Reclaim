//! [`ImageFile`] — a raw image on disk exposed as a read-only [`BlockSource`].
//!
//! This is the workhorse for CI and offline analysis: any `.img`/`.dmg`(raw)/`dd`
//! dump can be scanned exactly like a device. Container formats (DMG UDIF, VMDK,
//! E01, …) arrive in Phase 4 as their own sources.

use crate::error::BlockError;
use crate::open_log;
use crate::source::{BlockSource, ReadResult, SourceId};
use crate::DEFAULT_SECTOR_SIZE;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// A raw image file opened read-only.
#[derive(Debug)]
pub struct ImageFile {
    file: File,
    len: u64,
    sector_size: u32,
    physical_sector_size: u32,
    id: SourceId,
}

impl ImageFile {
    /// Open `path` read-only with the default 512-byte sector size.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BlockError> {
        Self::open_with_sector_size(path, DEFAULT_SECTOR_SIZE)
    }

    /// Open `path` read-only, declaring a logical `sector_size` (must be > 0).
    pub fn open_with_sector_size(
        path: impl AsRef<Path>,
        sector_size: u32,
    ) -> Result<Self, BlockError> {
        if sector_size == 0 {
            return Err(BlockError::Geometry("sector size must be non-zero".into()));
        }
        let path: &Path = path.as_ref();
        let file = File::open(path).map_err(|e| BlockError::io(path.to_string_lossy(), e))?;
        // File::open is O_RDONLY (plus O_CLOEXEC). Record for --prove-readonly.
        open_log::record_open(path, libc::O_RDONLY);
        let meta = file
            .metadata()
            .map_err(|e| BlockError::io(path.to_string_lossy(), e))?;
        let len = meta.len();
        let canon: PathBuf = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let id = SourceId::for_image(&canon.to_string_lossy(), len);
        Ok(ImageFile {
            file,
            len,
            sector_size,
            physical_sector_size: sector_size,
            id,
        })
    }
}

impl BlockSource for ImageFile {
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
        read_file_at(&self.file, self.len, self.sector_size, offset, buf)
    }

    fn id(&self) -> SourceId {
        self.id.clone()
    }
}

/// Shared pread-based fill used by [`ImageFile`]. Fills `buf` completely; bytes
/// beyond `len` or that error are left zero and their sectors marked bad.
pub(crate) fn read_file_at(
    file: &File,
    len: u64,
    sector_size: u32,
    offset: u64,
    buf: &mut [u8],
) -> ReadResult {
    let ss = u64::from(sector_size);
    let sector_count = buf.len().div_ceil(sector_size as usize);
    let mut res = ReadResult::good(sector_size, sector_count);
    buf.fill(0);

    if buf.is_empty() {
        return res;
    }
    // Unaligned offset: mark everything bad (mirrors raw-device semantics).
    if !offset.is_multiple_of(ss) {
        for i in 0..sector_count {
            res.mark_bad(i);
        }
        return res;
    }
    if offset >= len {
        for i in 0..sector_count {
            res.mark_bad(i);
        }
        return res;
    }

    let avail = core::cmp::min(buf.len() as u64, len - offset) as usize;

    // Read the available prefix; on error, mark from the failure point onward.
    let mut done = 0usize;
    while done < avail {
        let Some(dst) = buf.get_mut(done..avail) else {
            break;
        };
        match file.read_at(dst, offset + done as u64) {
            Ok(0) => break, // short of `avail`: treat remainder as bad below
            Ok(n) => done += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }

    // Any byte position in `[done, buf.len())` is missing/bad.
    mark_bad_from_byte(&mut res, sector_size, buf.len(), done);
    res
}

/// Mark every sector that contains an in-buffer byte at position `>= start_byte`
/// as bad. `buf_len` is the actual request length so the final (possibly
/// partial) sector is only flagged if its *real* bytes are unread — a full read
/// whose length is not a sector multiple must still report all-good.
pub(crate) fn mark_bad_from_byte(
    res: &mut ReadResult,
    sector_size: u32,
    buf_len: usize,
    start_byte: usize,
) {
    if start_byte >= buf_len {
        return;
    }
    let ss = sector_size as usize;
    for i in 0..res.sector_count() {
        let raw_end = i.saturating_add(1).saturating_mul(ss);
        let sector_end = core::cmp::min(raw_end, buf_len);
        if sector_end > start_byte {
            res.mark_bad(i);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::source::SectorStatus;
    use std::io::Write;

    fn temp_image(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn reads_full_and_reports_good() {
        let data: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        let tmp = temp_image(&data);
        let img = ImageFile::open(tmp.path()).unwrap();
        assert_eq!(img.len(), 2048);
        assert_eq!(img.sector_size(), 512);

        let mut buf = vec![0u8; 1024];
        let r = img.read_at(512, &mut buf);
        assert!(r.all_good());
        assert_eq!(&buf[..], &data[512..1536]);
    }

    #[test]
    fn read_past_eof_marks_bad_and_zeroes() {
        let data = vec![0xABu8; 1024];
        let tmp = temp_image(&data);
        let img = ImageFile::open(tmp.path()).unwrap();

        // Request 1024 bytes starting at 512: only 512 real bytes remain.
        let mut buf = vec![0x11u8; 1024];
        let r = img.read_at(512, &mut buf);
        assert!(r.any_bad());
        // First sector good (real data), second sector bad (beyond EOF, zeroed).
        assert_eq!(r.status(0), SectorStatus::Good);
        assert_eq!(r.status(1), SectorStatus::Bad);
        assert_eq!(&buf[..512], &data[512..1024]);
        assert!(buf[512..].iter().all(|b| *b == 0));
    }

    #[test]
    fn offset_at_or_after_len_all_bad() {
        let tmp = temp_image(&vec![7u8; 512]);
        let img = ImageFile::open(tmp.path()).unwrap();
        let mut buf = vec![9u8; 512];
        let r = img.read_at(512, &mut buf);
        assert!(!r.all_good());
        assert_eq!(r.bad_count(), 1);
        assert!(buf.iter().all(|b| *b == 0));
    }

    #[test]
    fn unaligned_offset_is_bad() {
        let tmp = temp_image(&vec![1u8; 2048]);
        let img = ImageFile::open(tmp.path()).unwrap();
        let mut buf = vec![0u8; 512];
        let r = img.read_at(100, &mut buf); // 100 % 512 != 0
        assert!(!r.all_good());
        assert_eq!(r.bad_count(), 1);
    }
}
