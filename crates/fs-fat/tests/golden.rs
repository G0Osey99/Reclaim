//! Integration test against the real FAT32 golden image (skips in CI).

use reclaim_block::{BlockSource, ImageFile, OffsetView};
use reclaim_fs_core::{EntryState, FileSystem, VecSink, WalkOpts};
use std::path::Path;
use std::sync::Arc;

#[test]
fn walks_deleted_files_from_fat32_golden_image() {
    let path = Path::new("../../testdata/build/fat32-usb-delete.img");
    if !path.exists() {
        eprintln!("skipping: golden image not built ({})", path.display());
        return;
    }
    let img = ImageFile::open(path).expect("open image");
    let src: Arc<dyn BlockSource> = Arc::new(img);

    let map = reclaim_part::scan(&src);
    let (start, len) = map
        .entries
        .first()
        .map(|p| (p.start, p.len))
        .unwrap_or((0, src.len()));

    let view: Arc<dyn BlockSource> =
        Arc::new(OffsetView::new(Arc::clone(&src), start, len).unwrap());
    let probe = fs_fat::probe(&view).expect("fat probe");
    eprintln!("probe kind={} cluster={}", probe.fs_kind, probe.block_size);
    assert_eq!(probe.fs_kind, "fat32");

    let fs = fs_fat::open(Arc::clone(&view), probe).expect("open fat");
    let mut sink = VecSink::default();
    let stats = fs.walk(&mut sink, &WalkOpts::default()).expect("walk");
    eprintln!(
        "emitted={} live={} deleted={}",
        stats.emitted, stats.live, stats.deleted
    );

    let deleted: Vec<_> = sink
        .entries
        .iter()
        .filter(|e| e.state == EntryState::Deleted && !e.name.starts_with("._"))
        .collect();
    for e in &deleted {
        eprintln!(
            "  DELETED {} size={} assumed={} conf={:.2}",
            e.path.as_deref().unwrap_or(&e.name),
            e.size,
            e.contiguous_assumed,
            e.confidence
        );
    }
    // Recipe deletes indices [2,5,7,11,13,17,19,23] → file_00X names.
    assert!(
        deleted.iter().any(|e| e.name == "file_002.txt"),
        "expected recovered name file_002.txt (first char inferred); got {:?}",
        deleted.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
    assert!(
        deleted.len() >= 6,
        "expected several deleted user files, got {}",
        deleted.len()
    );
}
