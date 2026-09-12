//! [`RawDevice`] — a raw character device (`/dev/rdiskN` on macOS) opened
//! `O_RDONLY` and read with `pread(2)` (docs/plan/06 §2).
//!
//! Raw (`rdisk`) nodes are unbuffered and require sector-aligned reads; they are
//! several times faster than the buffered `disk` node and do not hammer a
//! failing drive with read-ahead. A hard read error (`EIO`) on a sector is
//! turned into a [`crate::SectorStatus::Bad`] sector — a value, not an
//! exception (docs/plan/03 §6) — so a scan continues past media defects.
//!
//! On non-macOS targets the same code compiles and works against a regular file
//! (geometry falls back to `fstat` + a 512-byte sector), which keeps Linux CI
//! and the unit tests honest without needing a real device.

use crate::error::BlockError;
use crate::image_file::mark_bad_from_byte;
use crate::open_log;
use crate::source::{BlockSource, ReadResult, SourceId};
use crate::DEFAULT_SECTOR_SIZE;
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

/// A raw block/character device (or a regular file) opened read-only.
#[derive(Debug)]
pub struct RawDevice {
    fd: OwnedFd,
    len: u64,
    sector_size: u32,
    physical_sector_size: u32,
    id: SourceId,
}

impl RawDevice {
    /// Open `path` read-only. On macOS prefer the raw node (`/dev/rdiskN`).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BlockError> {
        let path: &Path = path.as_ref();
        let cpath = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| BlockError::Geometry("path contains a NUL byte".into()))?;

        // O_RDONLY (value 0) — the only mode this crate ever uses on a source.
        let flags = libc::O_RDONLY | libc::O_CLOEXEC;
        // SAFETY: cpath is a valid NUL-terminated C string for the call's duration.
        let raw: RawFd = unsafe { libc::open(cpath.as_ptr(), flags) };
        if raw < 0 {
            return Err(BlockError::io(
                path.to_string_lossy(),
                std::io::Error::last_os_error(),
            ));
        }
        open_log::record_open(path, libc::O_RDONLY);
        // SAFETY: `raw` is a fresh, owned fd from a successful open().
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        let (len, sector_size, physical_sector_size) = query_geometry(fd.as_raw_fd())
            .unwrap_or_else(|| {
                let len = fstat_len(fd.as_raw_fd()).unwrap_or(0);
                (len, DEFAULT_SECTOR_SIZE, DEFAULT_SECTOR_SIZE)
            });

        if sector_size == 0 {
            return Err(BlockError::Geometry(
                "device reported zero sector size".into(),
            ));
        }

        let bsd = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let id = SourceId::for_device(&bsd, len);

        Ok(RawDevice {
            fd,
            len,
            sector_size,
            physical_sector_size,
            id,
        })
    }
}

impl BlockSource for RawDevice {
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

        let avail = core::cmp::min(buf.len() as u64, self.len - offset) as usize;
        let fd = self.fd.as_raw_fd();

        // Fast path: one pread of the whole available region. Only on a hard
        // error do we drop to per-sector isolation, so healthy reads stay fast.
        let mut done = 0usize;
        while done < avail {
            let Some(dst) = buf.get_mut(done..avail) else {
                break;
            };
            match pread_raw(fd, dst, offset + done as u64) {
                Ok(0) => break, // short read; remainder handled as bad below
                Ok(n) => done += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    // Isolate the offending sector, mark it bad, and continue.
                    let sec = done / ss_usize;
                    let sec_start = sec * ss_usize;
                    let sec_end = core::cmp::min(sec_start + ss_usize, avail);
                    let recovered = match buf.get_mut(sec_start..sec_end) {
                        Some(sdst) => pread_raw(fd, sdst, offset + sec_start as u64).ok(),
                        None => None,
                    };
                    match recovered {
                        Some(n) if n > 0 => {
                            done = sec_start + n;
                        }
                        _ => {
                            res.mark_bad(sec);
                            // Re-zero the sector we could not read.
                            if let Some(sdst) = buf.get_mut(sec_start..sec_end) {
                                sdst.fill(0);
                            }
                            done = sec_end;
                        }
                    }
                }
            }
        }

        // Bytes in `[done, buf.len())` were never read: the failed tail (if the
        // loop stopped early) plus anything beyond EOF. Individually-isolated bad
        // sectors in `[0, done)` were already marked above.
        mark_bad_from_byte(&mut res, self.sector_size, buf.len(), done);
        res
    }

    fn id(&self) -> SourceId {
        self.id.clone()
    }
}

/// `pread(2)` wrapper: returns bytes read (0 = EOF) or the OS error.
fn pread_raw(fd: RawFd, dst: &mut [u8], off: u64) -> std::io::Result<usize> {
    // SAFETY: dst is a valid, writable slice of dst.len() bytes; fd is open.
    let ret = unsafe {
        libc::pread(
            fd,
            dst.as_mut_ptr().cast::<libc::c_void>(),
            dst.len(),
            off as libc::off_t,
        )
    };
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(ret as usize)
    }
}

/// `fstat(2)` size in bytes.
fn fstat_len(fd: RawFd) -> Option<u64> {
    // SAFETY: zeroed stat is valid to pass to fstat, which fully initializes it.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::fstat(fd, &mut st) };
    if r != 0 {
        return None;
    }
    Some(st.st_size.max(0) as u64)
}

/// Query device geometry via the macOS `DKIOC` ioctls. Returns
/// `(len_bytes, logical_sector, physical_sector)`.
#[cfg(target_os = "macos")]
fn query_geometry(fd: RawFd) -> Option<(u64, u32, u32)> {
    // From <sys/disk.h>: _IOR('d', n, T). Read-only geometry queries.
    const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x4004_6418; // n=24, u32
    const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x4008_6419; // n=25, u64
    const DKIOCGETPHYSICALBLOCKSIZE: libc::c_ulong = 0x4004_644d; // n=77, u32

    let mut bs: u32 = 0;
    let mut count: u64 = 0;
    // SAFETY: each ioctl writes into a correctly-typed local via a raw pointer.
    let r1 = unsafe { libc::ioctl(fd, DKIOCGETBLOCKSIZE, &mut bs as *mut u32) };
    let r2 = unsafe { libc::ioctl(fd, DKIOCGETBLOCKCOUNT, &mut count as *mut u64) };
    if r1 != 0 || r2 != 0 || bs == 0 {
        return None;
    }
    let mut pbs: u32 = 0;
    let r3 = unsafe { libc::ioctl(fd, DKIOCGETPHYSICALBLOCKSIZE, &mut pbs as *mut u32) };
    let physical = if r3 == 0 && pbs != 0 { pbs } else { bs };
    Some((count.saturating_mul(u64::from(bs)), bs, physical))
}

#[cfg(not(target_os = "macos"))]
fn query_geometry(_fd: RawFd) -> Option<(u64, u32, u32)> {
    // Linux/other: fall back to fstat + default sector (see caller). RawDevice
    // is macOS-first; full Linux geometry (BLKGETSIZE64/BLKSSZGET) is future work.
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn reads_regular_file_as_device() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 97) as u8).collect();
        let tmp = temp(&data);
        let dev = RawDevice::open(tmp.path()).unwrap();
        assert_eq!(dev.len(), 4096);
        assert_eq!(dev.sector_size(), 512);

        let mut buf = vec![0u8; 2048];
        let r = dev.read_at(1024, &mut buf);
        assert!(r.all_good());
        assert_eq!(&buf[..], &data[1024..3072]);
    }

    #[test]
    fn past_eof_is_bad() {
        let tmp = temp(&vec![0x5Au8; 1024]);
        let dev = RawDevice::open(tmp.path()).unwrap();
        let mut buf = vec![0u8; 1024];
        let r = dev.read_at(512, &mut buf);
        assert!(r.any_bad());
        assert_eq!(&buf[..512], &vec![0x5Au8; 512][..]);
        assert!(buf[512..].iter().all(|b| *b == 0));
    }
}
