//! The [`BlockSource`] trait and its supporting types.

use crate::bitset::Bitset;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A stable identity for a source, persisted in session files so a scan can be
/// re-attached to the same device/image across runs (docs/plan/07 §4).
///
/// The string is a descriptor such as `image:/abs/path:<len>` or
/// `device:disk4:<len>`. It is intentionally coarse; sessions also record
/// first/last-MiB hashes to disambiguate.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceId(String);

impl SourceId {
    /// Wrap an arbitrary descriptor string.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        SourceId(s.into())
    }

    /// Identity for an image file at `path` of `len` bytes.
    #[must_use]
    pub fn for_image(path: &str, len: u64) -> Self {
        SourceId(format!("image:{path}:{len}"))
    }

    /// Identity for a raw device (`bsd` e.g. `disk4` / `rdisk4`) of `len` bytes.
    #[must_use]
    pub fn for_device(bsd: &str, len: u64) -> Self {
        SourceId(format!("device:{bsd}:{len}"))
    }

    /// The descriptor string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Per-sector outcome of a read.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SectorStatus {
    /// Bytes are the real device/image contents.
    Good,
    /// Sector could not be read (I/O error / EIO); its bytes are zero-filled.
    Bad,
    /// Sector was never read (e.g. an unimaged region of a `MappedImage`);
    /// its bytes are zero-filled. Reserved for Phase 1 imaging.
    Unread,
}

/// Result of [`BlockSource::read_at`]: the buffer is always filled to its full
/// length (bad/unread bytes are zeroed), and this describes which sectors are
/// trustworthy.
///
/// Sectors are `sector_size` bytes. Sector `i` covers request bytes
/// `[i*sector_size, min((i+1)*sector_size, buf.len()))` and, for a request that
/// began at byte `offset` (which must be sector-aligned), corresponds to
/// LBA `offset / sector_size + i`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadResult {
    sector_size: u32,
    sector_count: usize,
    bad: Bitset,
    unread: Bitset,
}

impl ReadResult {
    /// A result where every sector is [`SectorStatus::Good`].
    #[must_use]
    pub fn good(sector_size: u32, sector_count: usize) -> Self {
        ReadResult {
            sector_size,
            sector_count,
            bad: Bitset::new(sector_count),
            unread: Bitset::new(sector_count),
        }
    }

    /// The sector size used for this result's bitmaps.
    #[must_use]
    pub fn sector_size(&self) -> u32 {
        self.sector_size
    }

    /// Number of sectors covered by the request.
    #[must_use]
    pub fn sector_count(&self) -> usize {
        self.sector_count
    }

    /// Mark sector `i` bad.
    pub fn mark_bad(&mut self, i: usize) {
        self.bad.set(i);
    }

    /// Mark sector `i` unread.
    pub fn mark_unread(&mut self, i: usize) {
        self.unread.set(i);
    }

    /// Status of sector `i`. Bad takes precedence over Unread; default Good.
    #[must_use]
    pub fn status(&self, i: usize) -> SectorStatus {
        if self.bad.get(i) {
            SectorStatus::Bad
        } else if self.unread.get(i) {
            SectorStatus::Unread
        } else {
            SectorStatus::Good
        }
    }

    /// True if every sector is good.
    #[must_use]
    pub fn all_good(&self) -> bool {
        !self.bad.any() && !self.unread.any()
    }

    /// True if any sector is bad.
    #[must_use]
    pub fn any_bad(&self) -> bool {
        self.bad.any()
    }

    /// True if any sector is unread.
    #[must_use]
    pub fn any_unread(&self) -> bool {
        self.unread.any()
    }

    /// Number of bad sectors.
    #[must_use]
    pub fn bad_count(&self) -> usize {
        self.bad.count_ones()
    }

    /// Iterate indices (within the request) of bad sectors.
    pub fn bad_sectors(&self) -> impl Iterator<Item = usize> + '_ {
        self.bad.iter_set()
    }

    /// Iterate indices (within the request) of unread sectors.
    pub fn unread_sectors(&self) -> impl Iterator<Item = usize> + '_ {
        self.unread.iter_set()
    }
}

/// A read-only, random-access block source (docs/plan/03 §2.1).
///
/// **There is no write method.** Implementations open their backing store
/// `O_RDONLY`; the imager writes to a separate destination `File`.
pub trait BlockSource: Send + Sync {
    /// Length of the source in bytes.
    fn len(&self) -> u64;

    /// True if the source is zero-length.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Logical sector size in bytes.
    fn sector_size(&self) -> u32;

    /// Physical sector size in bytes (>= logical; equal when unknown).
    fn physical_sector_size(&self) -> u32;

    /// Read exactly `buf.len()` bytes at `offset`. The buffer is always filled
    /// completely; bad/unread sectors are zero-filled and reported in the
    /// returned [`ReadResult`]. `offset` must be a multiple of
    /// [`BlockSource::sector_size`]; an unaligned offset yields an all-bad
    /// result.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult;

    /// Stable identity across sessions.
    fn id(&self) -> SourceId;

    /// True once a read has observed the backing device disappear (e.g. a
    /// hot-unplugged USB disk — `ENXIO`/`ENODEV`). Scans poll this to checkpoint
    /// and exit resumably instead of grinding through zero-filled reads
    /// (docs/plan/07 §4). Non-device sources never vanish.
    fn vanished(&self) -> bool {
        false
    }
}

/// Blanket impl so `Arc<dyn BlockSource>`, `Box<dyn BlockSource>`, `&T` etc.
/// are themselves `BlockSource` — lets views/caches wrap any handle uniformly.
impl<T: BlockSource + ?Sized> BlockSource for std::sync::Arc<T> {
    fn len(&self) -> u64 {
        (**self).len()
    }
    fn sector_size(&self) -> u32 {
        (**self).sector_size()
    }
    fn physical_sector_size(&self) -> u32 {
        (**self).physical_sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult {
        (**self).read_at(offset, buf)
    }
    fn id(&self) -> SourceId {
        (**self).id()
    }
    fn vanished(&self) -> bool {
        (**self).vanished()
    }
}

/// Round `value` down to a multiple of `align` (which must be non-zero).
#[must_use]
pub(crate) fn align_down(value: u64, align: u64) -> u64 {
    value - (value % align)
}

/// Round `value` up to a multiple of `align` (which must be non-zero).
/// Saturates rather than overflowing.
#[must_use]
pub(crate) fn align_up(value: u64, align: u64) -> u64 {
    match value % align {
        0 => value,
        rem => value.saturating_add(align - rem),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn align_helpers() {
        assert_eq!(align_down(0, 512), 0);
        assert_eq!(align_down(511, 512), 0);
        assert_eq!(align_down(512, 512), 512);
        assert_eq!(align_down(1025, 512), 1024);
        assert_eq!(align_up(0, 512), 0);
        assert_eq!(align_up(1, 512), 512);
        assert_eq!(align_up(512, 512), 512);
        assert_eq!(align_up(513, 512), 1024);
    }

    #[test]
    fn read_result_status() {
        let mut r = ReadResult::good(512, 8);
        assert!(r.all_good());
        r.mark_bad(3);
        r.mark_unread(5);
        assert!(!r.all_good());
        assert_eq!(r.status(0), SectorStatus::Good);
        assert_eq!(r.status(3), SectorStatus::Bad);
        assert_eq!(r.status(5), SectorStatus::Unread);
        assert_eq!(r.bad_count(), 1);
        assert_eq!(r.bad_sectors().collect::<Vec<_>>(), vec![3]);
        assert_eq!(r.unread_sectors().collect::<Vec<_>>(), vec![5]);
    }

    #[test]
    fn source_id_shapes() {
        assert_eq!(
            SourceId::for_image("/a/b.img", 100).as_str(),
            "image:/a/b.img:100"
        );
        assert_eq!(
            SourceId::for_device("disk4", 64).as_str(),
            "device:disk4:64"
        );
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn align_down_invariants(value in 0u64..=1u64 << 40, exp in 0u32..=16) {
            let align = 1u64 << exp; // power-of-two sector sizes
            let d = align_down(value, align);
            prop_assert!(d <= value);
            prop_assert_eq!(d % align, 0);
            prop_assert!(value - d < align);
        }

        #[test]
        fn align_up_invariants(value in 0u64..=1u64 << 40, exp in 0u32..=16) {
            let align = 1u64 << exp;
            let u = align_up(value, align);
            prop_assert!(u >= value);
            prop_assert_eq!(u % align, 0);
            prop_assert!(u - value < align);
        }

        #[test]
        fn sector_count_matches_div_ceil(len in 0usize..1_000_000, exp in 9u32..=12) {
            // The sector-count math used across every source.
            let ss = 1usize << exp;
            let count = len.div_ceil(ss);
            if len == 0 {
                prop_assert_eq!(count, 0);
            } else {
                prop_assert!((count - 1) * ss < len);
                prop_assert!(count * ss >= len);
            }
        }
    }
}
