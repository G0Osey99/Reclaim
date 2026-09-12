//! [`FaultInjector`] — a test wrapper that reports chosen LBAs as bad
//! (docs/plan/09 §2 "damaged media" layer). Lets the imager and `Suspect`
//! handling be tested deterministically without physically damaged media.

use crate::source::{BlockSource, ReadResult, SourceId};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Wraps a source and forces the sectors at the given LBAs to read as bad
/// (zero-filled). LBAs are in units of the inner source's sector size.
pub struct FaultInjector {
    inner: Arc<dyn BlockSource>,
    bad_lbas: BTreeSet<u64>,
    id: SourceId,
}

impl std::fmt::Debug for FaultInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FaultInjector")
            .field("bad_lbas", &self.bad_lbas)
            .field("id", &self.id)
            .finish()
    }
}

impl FaultInjector {
    /// Wrap `inner`, marking `bad_lbas` as unreadable.
    pub fn new(inner: Arc<dyn BlockSource>, bad_lbas: impl IntoIterator<Item = u64>) -> Self {
        let bad_lbas: BTreeSet<u64> = bad_lbas.into_iter().collect();
        let id = SourceId::new(format!("fault:{}:{}", inner.id(), bad_lbas.len()));
        FaultInjector {
            inner,
            bad_lbas,
            id,
        }
    }

    /// The set of injected bad LBAs.
    #[must_use]
    pub fn bad_lbas(&self) -> &BTreeSet<u64> {
        &self.bad_lbas
    }
}

impl BlockSource for FaultInjector {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }

    fn physical_sector_size(&self) -> u32 {
        self.inner.physical_sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult {
        let ss = u64::from(self.sector_size().max(1));
        let ss_usize = self.sector_size().max(1) as usize;
        let mut res = self.inner.read_at(offset, buf);

        if !offset.is_multiple_of(ss) {
            return res; // inner already marked it bad
        }
        let first_lba = offset / ss;
        for i in 0..res.sector_count() {
            let lba = first_lba + i as u64;
            if self.bad_lbas.contains(&lba) {
                res.mark_bad(i);
                let start = i * ss_usize;
                let end = core::cmp::min(start + ss_usize, buf.len());
                if let Some(sect) = buf.get_mut(start..end) {
                    sect.fill(0);
                }
            }
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
    use crate::image_file::ImageFile;
    use crate::source::SectorStatus;
    use std::io::Write;

    #[test]
    fn injects_bad_sectors_and_zeroes() {
        let data: Vec<u8> = (0..4096u32).map(|_| 0xFFu8).collect();
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&data).unwrap();
        f.flush().unwrap();

        let img: Arc<dyn BlockSource> = Arc::new(ImageFile::open(f.path()).unwrap());
        // Inject bad at LBA 2 and 5 (512-byte sectors).
        let inj = FaultInjector::new(img, [2u64, 5]);

        let mut buf = vec![0u8; 4096]; // 8 sectors starting at LBA 0
        let r = inj.read_at(0, &mut buf);
        assert!(r.any_bad());
        assert_eq!(r.bad_count(), 2);
        assert_eq!(r.status(2), SectorStatus::Bad);
        assert_eq!(r.status(5), SectorStatus::Bad);
        assert_eq!(r.status(0), SectorStatus::Good);
        // Bad sectors zeroed; good sectors keep their 0xFF.
        assert!(buf[1024..1536].iter().all(|b| *b == 0));
        assert!(buf[2560..3072].iter().all(|b| *b == 0));
        assert!(buf[0..512].iter().all(|b| *b == 0xFF));
    }
}
