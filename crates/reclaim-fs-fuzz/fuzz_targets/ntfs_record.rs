#![no_main]
//! Fuzz the byte-level MFT record parser (fix-ups, attributes, data runs).
use libfuzzer_sys::fuzz_target;
use fs_ntfs::record::MftRecord;

fuzz_target!(|data: &[u8]| {
    let _ = MftRecord::parse(data, 1024);
});
