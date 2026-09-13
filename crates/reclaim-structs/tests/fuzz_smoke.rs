//! Randomized no-panic smoke test for the lost-structure sweep + every anchor
//! validator. CI-runnable stand-in for cargo-fuzz (docs/plan/09 §4); the
//! libFuzzer target lives in `crates/reclaim-fs-fuzz`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use reclaim_block::{BlockSource, MemorySource};
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
    let _ = reclaim_structs::scan(&src);
}

#[test]
fn structs_scan_never_panics() {
    let mut rng = Rng(0x5747_5054_1234_9F01);
    // Random images.
    for _ in 0..1500 {
        let n = (rng.next() % 262_144) as usize + 64;
        run(rng.bytes(n));
    }
    // Seeded with each magic sprinkled at plausible offsets to exercise the
    // validators (they must reject implausible geometry without panicking).
    let magics: &[(&[u8], usize)] = &[
        (b"NXSB", 32),
        (b"APSB", 32),
        (b"H+", 1024),
        (b"NTFS    ", 3),
        (b"EXFAT   ", 3),
        (b"FAT32   ", 82),
        (&[0x53, 0xEF], 1080),
        (b"XFSB", 0),
        (b"_BHRfS_M", 65600),
        (b"CD001", 32769),
        (&[0x0c, 0xb1, 0xba, 0x00], 0),
    ];
    for _ in 0..1000 {
        let mut b = rng.bytes(262_144);
        for (needle, off) in magics {
            let base =
                (rng.next() as usize) % (b.len().saturating_sub(needle.len() + off + 1)).max(1);
            let at = base + off;
            if at + needle.len() <= b.len() {
                b[at..at + needle.len()].copy_from_slice(needle);
            }
        }
        run(b);
    }
}
