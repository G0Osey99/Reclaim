#![no_main]
//! Fuzz the HFS+ byte-level parsers: catalog key, catalog record
//! (file/folder/thread), and fork data.
use libfuzzer_sys::fuzz_target;
use fs_hfs::record::{parse_key, parse_record, ForkData};

fuzz_target!(|data: &[u8]| {
    if let Some(key) = parse_key(data, 0) {
        let _ = parse_record(data, key.total_len);
    }
    let _ = parse_record(data, 0);
    let _ = ForkData::parse(data, 0);
});
