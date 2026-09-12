//! Randomized no-panic smoke test for the exFAT parser (boot sector + directory
//! walk). CI-runnable stand-in for cargo-fuzz (docs/plan/09 §4); the matching
//! libFuzzer target is under `fuzz/`.

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
    if let Some(p) = fs_exfat::probe(&src) {
        if let Ok(fs) = fs_exfat::open(Arc::clone(&src), p) {
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
fn exfat_never_panics() {
    let mut rng = Rng(0xC0FF_EE12_3456_789A);
    for _ in 0..3000 {
        let n = (rng.next() % 8192) as usize + 16;
        run(rng.bytes(n));
    }
    // Seeded: exFAT magic + plausible shifts, everything else random.
    for _ in 0..3000 {
        let mut b = rng.bytes(64 * 1024);
        b[3..11].copy_from_slice(b"EXFAT   ");
        b[108] = 9; // bytes/sector shift = 512
        b[109] = (rng.next() % 8) as u8; // sectors/cluster shift
        b[110] = 1; // number of fats
        b[510] = 0x55;
        b[511] = 0xAA;
        run(b);
    }
}
