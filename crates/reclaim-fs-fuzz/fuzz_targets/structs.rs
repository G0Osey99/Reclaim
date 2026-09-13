#![no_main]
//! Fuzz the lost-structure sweep + every filesystem anchor validator.
use libfuzzer_sys::fuzz_target;
use reclaim_block::{BlockSource, MemorySource};
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(data.to_vec()));
    let _ = reclaim_structs::scan(&src);
});
