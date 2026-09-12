#![no_main]
//! Fuzz target for the `elf` validator (docs/plan/09 §4): malformed bytes must
//! never panic (build guide Part 1.4 rule 3).
use libfuzzer_sys::fuzz_target;
use reclaim_carve::reader::MemReader;
use reclaim_carve::validators::{Ctx, elf};

fuzz_target!(|data: &[u8]| {
    let sig = reclaim_sigs::all()
        .iter()
        .find(|s| s.validator == Some("elf"))
        .or_else(|| reclaim_sigs::all().first());
    if let Some(sig) = sig {
        let r = MemReader::new(data);
        let ctx = Ctx { reader: &r, file_start: 0, max_len: 1 << 30, sig };
        let _ = elf::validate(&ctx);
    }
});
