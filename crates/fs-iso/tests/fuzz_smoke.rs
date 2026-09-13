//! Randomized no-panic smoke test for the ISO 9660 / Joliet parser — volume
//! descriptor scan and the directory-record tree walk. CI-runnable stand-in for
//! cargo-fuzz (docs/plan/09 §4); the libFuzzer target lives in
//! `crates/reclaim-fs-fuzz`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

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

fn run(bytes: Vec<u8>) {
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(bytes));
    if let Some(p) = fs_iso::probe(&src) {
        if let Ok(fs) = fs_iso::open(Arc::clone(&src), p) {
            let mut sink = VecSink::default();
            let opts = WalkOpts {
                max_entries: 5_000,
                ..Default::default()
            };
            let _ = fs.walk(&mut sink, &opts);
        }
    }
}

#[test]
fn iso_walk_never_panics() {
    let mut rng = Rng(0xCD00_1000_ABCD_0001);
    for _ in 0..1500 {
        let n = (rng.next() % 262_144) as usize + 64;
        run(rng.bytes(n));
    }
    // Seeded: CD001 PVD at sector 16 with a random-ish root directory record.
    for _ in 0..1500 {
        let mut b = rng.bytes(200_000);
        let pvd = 16 * 2048;
        if b.len() > pvd + 200 {
            b[pvd] = 1; // PVD type
            b[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
            b[pvd + 128..pvd + 130].copy_from_slice(&2048u16.to_le_bytes()); // block size LE
                                                                             // root record at +156: length 34, extent LBA 20 (both-endian), len.
            b[pvd + 156] = 34;
            b[pvd + 156 + 2..pvd + 156 + 6].copy_from_slice(&20u32.to_le_bytes());
            b[pvd + 156 + 10..pvd + 156 + 14].copy_from_slice(&2048u32.to_le_bytes());
            b[pvd + 156 + 25] = 0x02; // directory flag
        }
        run(b);
    }
}
