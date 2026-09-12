#![no_main]
//! Fuzz the HFS+/HFSX volume walk (volume header, catalog B-tree leaf chain,
//! stale-record scan) plus the allocation bitmap.
use libfuzzer_sys::fuzz_target;
use reclaim_block::{BlockSource, MemorySource};
use reclaim_fs_core::{FileSystem, VecSink, WalkOpts};
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    // Cap the input: the deleted-record scan sweeps the whole volume.
    let cap = data.len().min(1 << 20);
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(data[..cap].to_vec()));
    if let Some(p) = fs_hfs::probe(&src) {
        if let Ok(fs) = fs_hfs::open(Arc::clone(&src), p) {
            let mut sink = VecSink::default();
            let _ = fs.walk(&mut sink, &WalkOpts::default());
            let _ = fs.allocation_bitmap();
        }
    }
});
