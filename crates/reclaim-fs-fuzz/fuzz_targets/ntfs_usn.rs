#![no_main]
//! Fuzz the $UsnJrnl:$J and $I30 slack name miners.
use libfuzzer_sys::fuzz_target;
use fs_ntfs::usn;

fuzz_target!(|data: &[u8]| {
    let _ = usn::parse_usn_stream(data);
    let _ = usn::scan_indx_names(data);
});
