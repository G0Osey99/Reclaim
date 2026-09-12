//! Randomized no-panic smoke test for the FAT parser (BPB + directory walk).
//! CI-runnable stand-in for cargo-fuzz (docs/plan/09 §4).

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
    if let Some(p) = fs_fat::probe(&src) {
        if let Ok(fs) = fs_fat::open(Arc::clone(&src), p) {
            let mut sink = VecSink::default();
            let opts = WalkOpts {
                max_entries: 10_000,
                ..Default::default()
            };
            let _ = fs.walk(&mut sink, &opts);
            let _ = fs.allocation_bitmap();
        }
    }
}

#[test]
fn fat_never_panics() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF0);
    for _ in 0..3000 {
        let n = (rng.next() % 8192) as usize + 16;
        run(rng.bytes(n));
    }
    // Seeded: a plausible FAT BPB (bps=512), random rest, mutated.
    for _ in 0..3000 {
        let mut b = rng.bytes(128 * 1024);
        b[11] = 0x00;
        b[12] = 0x02; // bytes/sector = 512
        b[13] = 1 << ((rng.next() % 4) as u8); // sectors/cluster (power of two)
        b[14] = 32; // reserved sectors low
        b[15] = 0;
        b[16] = 2; // num fats
                   // total sectors 32 + FAT size 32 → small.
        b[36..40].copy_from_slice(&40u32.to_le_bytes()); // FAT32 size
        b[44..48].copy_from_slice(&2u32.to_le_bytes()); // root cluster
        b[32..36].copy_from_slice(&(rng.next() as u32).to_le_bytes()); // total sectors
        b[510] = 0x55;
        b[511] = 0xAA;
        run(b);
    }
}
