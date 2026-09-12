//! Randomized no-panic smoke test for the APFS parser: the container/volume
//! walk plus the byte-level object, B-tree-node and `j_*` record parsers. This
//! is the CI-runnable stand-in for cargo-fuzz (docs/plan/09 §4); the libFuzzer
//! targets live in `crates/reclaim-fs-fuzz`.

use fs_apfs::btree::Node;
use fs_apfs::obj::{fletcher64_valid, ObjPhys};
use fs_apfs::records::{parse_dir_rec, parse_file_extent, parse_inode, split_key};
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

fn run_container(bytes: Vec<u8>) {
    let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(bytes));
    if let Some(p) = fs_apfs::probe(&src) {
        if let Ok(fs) = fs_apfs::open(Arc::clone(&src), p) {
            let mut sink = VecSink::default();
            let opts = WalkOpts {
                max_entries: 5_000,
                ..Default::default()
            };
            let _ = fs.walk(&mut sink, &opts);
            let _ = fs.allocation_bitmap();
            let _ = fs.snapshots();
        }
    }
}

#[test]
fn container_walk_never_panics() {
    let mut rng = Rng(0xA9F5_1234_5678_9F01);
    // Random volumes of a few blocks.
    for _ in 0..1500 {
        let n = (rng.next() % 262_144) as usize + 32;
        run_container(rng.bytes(n));
    }
    // Seeded: a plausible NXSB header, random rest.
    for _ in 0..1500 {
        let mut b = rng.bytes(131_072);
        if b.len() >= 200 {
            b[32..36].copy_from_slice(b"NXSB");
            b[36..40].copy_from_slice(&4096u32.to_le_bytes()); // block size
            b[40..48].copy_from_slice(&32u64.to_le_bytes()); // block count
            b[104..108].copy_from_slice(&8u32.to_le_bytes()); // xp_desc_blocks
            b[112..120].copy_from_slice(&1u64.to_le_bytes()); // xp_desc_base
            b[160..168].copy_from_slice(&2u64.to_le_bytes()); // omap_oid
            b[180..184].copy_from_slice(&1u32.to_le_bytes()); // max_fs
            b[184..192].copy_from_slice(&3u64.to_le_bytes()); // fs_oid[0]
        }
        run_container(b);
    }
}

#[test]
fn record_parsers_never_panic() {
    let mut rng = Rng(0x0F0F_0F0F_2222_3333);
    for _ in 0..40_000 {
        let n = (rng.next() % 512) as usize;
        let b = rng.bytes(n);
        let _ = ObjPhys::parse(&b);
        let _ = fletcher64_valid(&b);
        let _ = Node::parse(b.clone());
        let (obj, _ty) = split_key(if b.len() >= 8 {
            u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
        } else {
            0
        });
        let _ = parse_inode(obj, &b);
        let _ = parse_dir_rec(obj, &b, &b, rng.next() & 1 == 0);
        let _ = parse_file_extent(obj, &b, &b);
    }
}
