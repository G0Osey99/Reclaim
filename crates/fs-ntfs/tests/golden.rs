//! Integration test against the synthetic NTFS golden image built by
//! `scripts/gen-fs-images` (skips in CI where the image is absent). Exact
//! content recall is checked by the benchmark (scripts/bench.sh); here we
//! verify the walk, the names, and that a deleted file's extent points at its
//! real bytes (correct magic + full size).

use reclaim_block::{BlockSource, ImageFile, OffsetView};
use reclaim_fs_core::{EntryState, FileSystem, VecSink, WalkOpts};
use std::path::Path;
use std::sync::Arc;

#[test]
fn walks_deleted_files_from_ntfs_golden_image() {
    let path = Path::new("../../testdata/build/ntfs-delete.img");
    if !path.exists() {
        eprintln!("skipping: golden image not built ({})", path.display());
        return;
    }
    let img = ImageFile::open(path).expect("open image");
    let src: Arc<dyn BlockSource> = Arc::new(img);

    let map = reclaim_part::scan(&src);
    let (start, len) = map
        .entries
        .iter()
        .find(|p| p.type_byte == Some(0x07))
        .map(|p| (p.start, p.len))
        .unwrap_or((0, src.len()));

    let view: Arc<dyn BlockSource> =
        Arc::new(OffsetView::new(Arc::clone(&src), start, len).unwrap());
    let probe = fs_ntfs::probe(&view).expect("ntfs probe");
    assert_eq!(probe.fs_kind, "ntfs");

    let fs = fs_ntfs::open(Arc::clone(&view), probe).expect("open ntfs");
    let mut sink = VecSink::default();
    let stats = fs.walk(&mut sink, &WalkOpts::default()).expect("walk");
    eprintln!(
        "emitted={} live={} deleted={}",
        stats.emitted, stats.live, stats.deleted
    );

    let deleted: Vec<_> = sink
        .entries
        .iter()
        .filter(|e| e.state == EntryState::Deleted && !e.name.starts_with('$'))
        .collect();
    for e in &deleted {
        eprintln!(
            "  DELETED {} size={} extents={} conf={:.2}",
            e.path.as_deref().unwrap_or(&e.name),
            e.size,
            e.extents.len(),
            e.confidence
        );
    }
    assert!(
        deleted.iter().any(|e| e.name == "file_001.png"),
        "expected file_001.png among deleted"
    );
    assert!(
        deleted.len() >= 6,
        "expected several deleted files, got {}",
        deleted.len()
    );

    // The deleted PNG's extent must point at a real PNG header and cover its size.
    let e = deleted
        .iter()
        .find(|e| e.name == "file_001.png")
        .expect("file_001.png");
    assert_eq!(
        e.extent_bytes(),
        e.size,
        "extents should cover the whole file"
    );
    let first = e.extents.first().expect("one extent");
    let mut head = vec![0u8; 8];
    view.read_at(first.offset, &mut head);
    assert_eq!(
        &head, b"\x89PNG\r\n\x1a\n",
        "extent should point at the PNG header"
    );
}
