//! Randomized no-panic smoke test for the partition parser (docs/plan/09 §4).
//!
//! `cargo-fuzz` is not installed in every environment, so this in-tree test is
//! the CI-runnable robustness proof: thousands of random and header-seeded byte
//! buffers are fed to `scan`, which must never panic (build guide Part 1.4
//! rule 3). The matching libFuzzer targets live under `fuzz/` for nightly runs.

use reclaim_block::{BlockSource, MemorySource};
use std::sync::Arc;

/// A tiny deterministic xorshift PRNG (no dev-dependency needed).
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

fn scan_bytes(bytes: Vec<u8>) {
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(bytes));
    let map = reclaim_part::scan(&src);
    // Exercise the accessors the planner uses.
    let _ = map.volume_windows(src.len());
    for p in &map.entries {
        assert!(p.start <= src.len().max(1) * 4);
    }
}

#[test]
fn scan_never_panics_on_random_or_seeded_bytes() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // Pure random buffers of varied sizes.
    for _ in 0..3000 {
        let n = (rng.next() % 4096) as usize + 8;
        scan_bytes(rng.bytes(n));
    }
    // Seeded: an MBR signature with garbage table, mutated.
    for _ in 0..2000 {
        let mut b = rng.bytes(2048);
        if b.len() > 512 {
            b[510] = 0x55;
            b[511] = 0xAA;
            // Random "GPT" header at LBA 1 sometimes.
            if rng.next() & 1 == 0 && b.len() > 520 {
                b[512..520].copy_from_slice(b"EFI PART");
            }
        }
        scan_bytes(b);
    }
    // Truncated buffers around the sector boundary.
    for n in [0usize, 1, 63, 64, 127, 511, 512, 513, 1023, 1024] {
        scan_bytes(vec![0x55u8; n]);
    }
}
