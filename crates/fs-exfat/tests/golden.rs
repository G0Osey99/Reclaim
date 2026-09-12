//! Integration test against the real exFAT golden image (skips in CI where the
//! generated images are absent — docs/plan/09 §2).

use reclaim_block::{BlockSource, ImageFile, OffsetView};
use reclaim_fs_core::{EntryState, FileSystem, VecSink, WalkOpts};
use std::path::Path;
use std::sync::Arc;

#[test]
fn walks_deleted_files_from_exfat_golden_image() {
    let path = Path::new("../../testdata/build/exfat-camera-delete.img");
    if !path.exists() {
        eprintln!("skipping: golden image not built ({})", path.display());
        return;
    }
    let img = ImageFile::open(path).expect("open image");
    let src: Arc<dyn BlockSource> = Arc::new(img);

    // Partition scheme → volume window.
    let map = reclaim_part::scan(&src);
    eprintln!("scheme={:?} entries={}", map.scheme, map.entries.len());
    let (start, len) = map
        .entries
        .iter()
        .find(|p| p.fs_hint == Some("exfat/ntfs"))
        .map(|p| (p.start, p.len))
        .unwrap_or((0, src.len()));
    eprintln!("volume at {start}..{}", start + len);

    let view: Arc<dyn BlockSource> =
        Arc::new(OffsetView::new(Arc::clone(&src), start, len).unwrap());
    let probe = fs_exfat::probe(&view).expect("exfat probe");
    eprintln!(
        "probe: kind={} conf={} cluster={} label={:?}",
        probe.fs_kind, probe.confidence, probe.block_size, probe.label
    );
    assert_eq!(probe.fs_kind, "exfat");

    let fs = fs_exfat::open(Arc::clone(&view), probe).expect("open exfat");
    let mut sink = VecSink::default();
    let stats = fs.walk(&mut sink, &WalkOpts::default()).expect("walk");
    eprintln!(
        "walk: emitted={} live={} deleted={}",
        stats.emitted, stats.live, stats.deleted
    );

    let deleted: Vec<_> = sink
        .entries
        .iter()
        .filter(|e| e.state == EntryState::Deleted)
        .collect();
    for e in &deleted {
        eprintln!(
            "  DELETED {} size={} extents={} assumed={}",
            e.path.as_deref().unwrap_or(&e.name),
            e.size,
            e.extents.len(),
            e.contiguous_assumed
        );
    }
    // The recipe deletes IMG_0000..IMG_0009 from DCIM/100RECLM.
    assert!(
        deleted.len() >= 8,
        "expected ~10 deleted files, got {}",
        deleted.len()
    );
    assert!(
        deleted
            .iter()
            .any(|e| e.path.as_deref().unwrap_or("").contains("IMG_0000")),
        "expected IMG_0000 among deleted entries"
    );
    // Live files should also be present with real paths.
    assert!(stats.live >= 10, "expected the surviving files too");
}
