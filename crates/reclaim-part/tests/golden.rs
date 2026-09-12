//! Integration test against the synthetic `gpt-deleted-partition` golden image
//! (skips in CI where it is not built): the primary GPT is wiped, so the parser
//! must recover the partition from the backup header (docs/plan/04 §1).

use reclaim_block::{BlockSource, ImageFile};
use std::path::Path;
use std::sync::Arc;

#[test]
fn recovers_partition_from_backup_gpt() {
    let path = Path::new("../../testdata/build/gpt-deleted-partition.img");
    if !path.exists() {
        eprintln!("skipping: golden image not built ({})", path.display());
        return;
    }
    let src: Arc<dyn BlockSource> = Arc::new(ImageFile::open(path).expect("open image"));
    let map = reclaim_part::scan(&src);
    assert_eq!(map.scheme, reclaim_part::Scheme::Gpt);
    assert_eq!(
        map.entries.len(),
        1,
        "backup GPT should yield the partition"
    );
    let p = &map.entries[0];
    assert_eq!(p.type_label, "Microsoft Basic Data");
    assert_eq!(p.name.as_deref(), Some("RECLAIMDATA"));
    assert!(
        map.notes.iter().any(|n| n.contains("backup")),
        "should note the backup fallback: {:?}",
        map.notes
    );
}
