#![no_main]
//! Fuzz the ext2/3/4 walk: superblock + backup scan, group descriptors, inode
//! extent/pointer decode, directory-slack recovery, jbd2 journal + 0xF30A scan.
use libfuzzer_sys::fuzz_target;
use reclaim_block::{BlockSource, MemorySource};
use reclaim_fs_core::{FileSystem, VecSink, WalkOpts};
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(data.to_vec()));
    if let Some(p) = fs_ext::probe(&src) {
        if let Ok(fs) = fs_ext::open(Arc::clone(&src), p) {
            let mut sink = VecSink::default();
            let opts = WalkOpts {
                max_entries: 20_000,
                ..Default::default()
            };
            let _ = fs.walk(&mut sink, &opts);
            let _ = fs.allocation_bitmap();
        }
    }
});
