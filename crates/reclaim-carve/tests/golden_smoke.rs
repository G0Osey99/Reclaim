//! Smoke test the carver against a Phase-0 golden image when present.
//! Skips cleanly in CI (images are generated, never committed — docs/plan/09 §2).

use reclaim_block::{BlockSource, ImageFile};
use reclaim_carve::engine::{CarveEngine, CarveOptions};
use reclaim_carve::Validity;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[test]
fn carves_deleted_files_from_exfat_golden_image() {
    let path = Path::new("../../testdata/build/exfat-camera-delete.img");
    if !path.exists() {
        eprintln!("skipping: golden image not built ({})", path.display());
        return;
    }
    let img = ImageFile::open(path).expect("open image");
    let src: Arc<dyn BlockSource> = Arc::new(img);
    let engine = CarveEngine::new();
    let block_size = engine.infer_block_size(&src);
    eprintln!("inferred block size: {block_size}");

    let opts = CarveOptions {
        block_size,
        ..Default::default()
    };
    let mut results = Vec::new();
    let cancel = AtomicBool::new(false);
    let outcome = engine.scan(
        &src,
        &opts,
        0,
        &mut |_p| {},
        &mut |cf| results.push(cf),
        &cancel,
    );
    assert!(!outcome.interrupted);

    let count = |fam: &str| results.iter().filter(|c| c.family == fam).count();
    let full = results
        .iter()
        .filter(|c| c.validity == Validity::Full)
        .count();
    eprintln!(
        "carved {} results ({} full): image={} video={} doc={}",
        results.len(),
        full,
        count("image"),
        count("video"),
        count("doc"),
    );
    // The recipe deletes 10 early files across jpeg/png/mp4/txt; the carver
    // should recover a healthy fraction from unallocated space.
    assert!(
        results.len() >= 5,
        "expected to carve several files, got {}",
        results.len()
    );
    assert!(count("image") >= 2, "expected image carves");
}
