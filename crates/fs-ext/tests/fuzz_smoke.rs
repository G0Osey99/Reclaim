//! Randomized no-panic smoke test for the ext2/3/4 parser — the superblock +
//! backup scan, group descriptors, inode extent/pointer decode, directory
//! slack recovery, jbd2 journal walk and the `0xF30A` scan. CI-runnable stand-in
//! for cargo-fuzz (docs/plan/09 §4); the libFuzzer target lives in
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
    if let Some(p) = fs_ext::probe(&src) {
        if let Ok(fs) = fs_ext::open(Arc::clone(&src), p) {
            let mut sink = VecSink::default();
            let opts = WalkOpts {
                max_entries: 5_000,
                ..Default::default()
            };
            let _ = fs.walk(&mut sink, &opts);
            let _ = fs.allocation_bitmap();
        }
    }
}

#[test]
fn ext_walk_never_panics() {
    let mut rng = Rng(0x51EF_5300_1234_9F01);
    // Random volumes (probe almost always rejects — fast).
    for _ in 0..2000 {
        let n = (rng.next() % 131_072) as usize + 64;
        run(rng.bytes(n));
    }
    // Seeded: a plausible ext superblock (magic 0xEF53 at 1080), random rest.
    // The libFuzzer target (crates/reclaim-fs-fuzz) does the deep run; this is a
    // fast CI no-panic stand-in, so keep the seeded count modest.
    for _ in 0..600 {
        let mut b = rng.bytes(131_072);
        if b.len() > 2048 {
            // s_log_block_size = 0 → 1 KiB blocks.
            b[1024 + 0x18..1024 + 0x1C].copy_from_slice(&0u32.to_le_bytes());
            b[1024 + 0x14..1024 + 0x18].copy_from_slice(&1u32.to_le_bytes()); // first_data_block
            b[1024 + 0x20..1024 + 0x24].copy_from_slice(&8192u32.to_le_bytes()); // blocks/group
            b[1024 + 0x28..1024 + 0x2C]
                .copy_from_slice(&(rng.next() as u32 % 4096 + 16).to_le_bytes()); // inodes/group
            b[1024..1024 + 4].copy_from_slice(&256u32.to_le_bytes()); // inodes_count
            b[1024 + 0x04..1024 + 0x08].copy_from_slice(&256u32.to_le_bytes()); // blocks_count
            b[1024 + 0x38..1024 + 0x3A].copy_from_slice(&0xEF53u16.to_le_bytes()); // magic
            b[1024 + 0x4C..1024 + 0x50].copy_from_slice(&1u32.to_le_bytes()); // rev 1
            b[1024 + 0x58..1024 + 0x5A].copy_from_slice(&256u16.to_le_bytes()); // inode size
        }
        run(b);
    }
}
