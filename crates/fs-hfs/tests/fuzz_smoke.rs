//! Randomized no-panic smoke test for the HFS+ parser: the volume walk plus the
//! byte-level catalog key/record and fork parsers. CI-runnable stand-in for
//! cargo-fuzz (docs/plan/09 §4); the libFuzzer targets live in
//! `crates/reclaim-fs-fuzz`.

use fs_hfs::record::{parse_key, parse_record, ForkData};
use reclaim_block::{BlockSource, MemorySource};
use reclaim_fs_core::{FileSystem, VecSink, WalkOpts};
use std::sync::Arc;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() & 0xFF) as u8).collect()
    }
}

fn run_volume(bytes: Vec<u8>) {
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(bytes));
    if let Some(p) = fs_hfs::probe(&src) {
        if let Ok(fs) = fs_hfs::open(Arc::clone(&src), p) {
            let mut sink = VecSink::default();
            let _ = fs.walk(&mut sink, &WalkOpts::default());
            let _ = fs.allocation_bitmap();
        }
    }
}

#[test]
fn volume_walk_never_panics() {
    let mut rng = Rng(0x7777_8888_9999_BBBB);
    // Keep inputs small: the deleted-record scan sweeps the whole input.
    for _ in 0..1500 {
        let n = (rng.next() % 65_536) as usize + 16;
        run_volume(rng.bytes(n));
    }
    // Seeded: a plausible H+ volume header at +1024, random rest.
    for _ in 0..1500 {
        let mut b = rng.bytes(131_072);
        if b.len() >= 1024 + 400 {
            b[1024..1026].copy_from_slice(&0x482Bu16.to_be_bytes()); // "H+"
            b[1024 + 40..1024 + 44].copy_from_slice(&4096u32.to_be_bytes()); // block size
            b[1024 + 44..1024 + 48].copy_from_slice(&16u32.to_be_bytes()); // total blocks
                                                                           // catalog fork @ +272: one extent (start 4, count 4).
            b[1024 + 272 + 16..1024 + 272 + 20].copy_from_slice(&4u32.to_be_bytes());
            b[1024 + 272 + 20..1024 + 272 + 24].copy_from_slice(&4u32.to_be_bytes());
        }
        run_volume(b);
    }
}

#[test]
fn record_parsers_never_panic() {
    let mut rng = Rng(0x1234_ABCD_5678_EF01);
    for _ in 0..40_000 {
        let n = (rng.next() % 1024) as usize;
        let b = rng.bytes(n);
        if let Some(key) = parse_key(&b, 0) {
            let _ = parse_record(&b, key.total_len);
        }
        let _ = parse_record(&b, 0);
        let _ = ForkData::parse(&b, 0);
    }
}
