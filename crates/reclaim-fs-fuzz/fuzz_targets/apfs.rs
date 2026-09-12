#![no_main]
//! Fuzz the APFS container/volume walk (nx_superblock, checkpoint ring, object
//! map, FS tree) plus the space-manager bitmap and snapshot enumeration.
use libfuzzer_sys::fuzz_target;
use reclaim_block::{BlockSource, MemorySource};
use reclaim_fs_core::{FileSystem, VecSink, WalkOpts};
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(data.to_vec()));
    if let Some(p) = fs_apfs::probe(&src) {
        if let Ok(fs) = fs_apfs::open(Arc::clone(&src), p) {
            let mut sink = VecSink::default();
            let opts = WalkOpts { max_entries: 20_000, ..Default::default() };
            let _ = fs.walk(&mut sink, &opts);
            let _ = fs.allocation_bitmap();
            let _ = fs.snapshots();
        }
    }
});
