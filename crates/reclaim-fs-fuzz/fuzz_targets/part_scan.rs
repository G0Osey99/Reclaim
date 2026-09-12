#![no_main]
//! Fuzz the partition parser (GPT/MBR/EBR/APM). Malformed bytes must not panic.
use libfuzzer_sys::fuzz_target;
use reclaim_block::{BlockSource, MemorySource};
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(data.to_vec()));
    let map = reclaim_part::scan(&src);
    let _ = map.volume_windows(src.len());
});
