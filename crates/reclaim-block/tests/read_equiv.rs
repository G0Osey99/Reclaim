//! Property tests over the public block API: every source implementation must
//! return the same bytes as a direct slice of the backing data for arbitrary
//! sector-aligned reads, and bad-sector reporting must line up with injected
//! faults. This is the read-path correctness net for Phase 0.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]

use proptest::prelude::*;
use reclaim_block::{BlockSource, FaultInjector, ImageFile, OffsetView, ReadAheadCache};
use std::io::Write;
use std::sync::Arc;

fn make_image(data: &[u8], sector_size: u32) -> (tempfile::NamedTempFile, ImageFile) {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(data).unwrap();
    f.flush().unwrap();
    let img = ImageFile::open_with_sector_size(f.path(), sector_size).unwrap();
    (f, img)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, failure_persistence: None, ..ProptestConfig::default() })]

    /// ImageFile reads match the underlying bytes for aligned requests within len.
    #[test]
    fn image_reads_match(
        data in proptest::collection::vec(any::<u8>(), 512..16384),
        sector_exp in 9u32..=12,
        off_sel in 0u64..100_000,
        len_bytes in 1usize..4096,
    ) {
        let ss = 1u32 << sector_exp;
        let (_tmp, img) = make_image(&data, ss);
        // Choose a sector-aligned offset strictly inside the image.
        let max_sectors = (img.len() / u64::from(ss)).max(1);
        let off = (off_sel % max_sectors) * u64::from(ss);
        prop_assume!(off < img.len());
        let want_len = core::cmp::min(len_bytes as u64, img.len() - off) as usize;
        prop_assume!(want_len > 0);

        let mut buf = vec![0u8; want_len];
        let r = img.read_at(off, &mut buf);
        prop_assert!(r.all_good());
        let start = off as usize;
        prop_assert_eq!(&buf[..], &data[start..start + want_len]);
    }

    /// A read-ahead cache is transparent: same bytes as reading the image directly.
    #[test]
    fn cache_is_transparent(
        data in proptest::collection::vec(any::<u8>(), 4096..40000),
        off in 0usize..4096,
        len_bytes in 1usize..8192,
    ) {
        let ss = 512u32;
        let (_tmp, img) = make_image(&data, ss);
        let len = img.len();
        let img: Arc<dyn BlockSource> = Arc::new(img);
        let cache = ReadAheadCache::with_capacity(img.clone(), 2);

        let off = (off / 512 * 512) as u64; // align
        prop_assume!(off < len);
        let want = core::cmp::min(len_bytes as u64, len - off) as usize;
        prop_assume!(want > 0);

        let mut a = vec![0u8; want];
        let mut b = vec![0u8; want];
        let ra = img.read_at(off, &mut a);
        let rb = cache.read_at(off, &mut b);
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(ra.all_good(), rb.all_good());
    }

    /// An offset view returns exactly the parent's windowed bytes.
    #[test]
    fn offset_view_windows(
        data in proptest::collection::vec(any::<u8>(), 8192..20000),
        win_start_sectors in 0usize..8,
        read_off_sectors in 0usize..4,
        len_bytes in 1usize..2048,
    ) {
        let ss = 512u32;
        let (_tmp, img) = make_image(&data, ss);
        let img: Arc<dyn BlockSource> = Arc::new(img);
        let win_start = (win_start_sectors as u64) * 512;
        let win_len = 4096u64;
        prop_assume!(win_start + win_len <= data.len() as u64);
        let view = OffsetView::new(img, win_start, win_len).unwrap();

        let read_off = (read_off_sectors as u64) * 512;
        prop_assume!(read_off < win_len);
        let want = core::cmp::min(len_bytes as u64, win_len - read_off) as usize;
        prop_assume!(want > 0);

        let mut buf = vec![0u8; want];
        let r = view.read_at(read_off, &mut buf);
        prop_assert!(r.all_good());
        let abs = (win_start + read_off) as usize;
        prop_assert_eq!(&buf[..], &data[abs..abs + want]);
    }

    /// Injected faults surface as bad sectors and zeroed bytes exactly.
    #[test]
    fn fault_injection_marks_expected(
        bad_lbas in proptest::collection::hash_set(0u64..16, 0..6),
    ) {
        let ss = 512u32;
        let data = vec![0xC3u8; 16 * 512];
        let (_tmp, img) = make_image(&data, ss);
        let img: Arc<dyn BlockSource> = Arc::new(img);
        let inj = FaultInjector::new(img, bad_lbas.iter().copied());

        let mut buf = vec![0u8; 16 * 512];
        let r = inj.read_at(0, &mut buf);
        let got: std::collections::HashSet<u64> =
            r.bad_sectors().map(|s| s as u64).collect();
        prop_assert_eq!(got, bad_lbas.clone());
        for lba in 0..16usize {
            let slice = &buf[lba * 512..(lba + 1) * 512];
            if bad_lbas.contains(&(lba as u64)) {
                prop_assert!(slice.iter().all(|b| *b == 0));
            } else {
                prop_assert!(slice.iter().all(|b| *b == 0xC3));
            }
        }
    }
}
