//! Randomized no-panic smoke test for the NTFS parser: boot sector + MFT walk,
//! plus the byte-level MFT record and USN/$I30 parsers. CI-runnable stand-in for
//! cargo-fuzz (docs/plan/09 §4).

use fs_ntfs::record::MftRecord;
use fs_ntfs::usn;
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
    if let Some(p) = fs_ntfs::probe(&src) {
        if let Ok(fs) = fs_ntfs::open(Arc::clone(&src), p) {
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
fn ntfs_record_parser_never_panics() {
    let mut rng = Rng(0xABCD_1234_5678_9F01);
    for _ in 0..20_000 {
        let n = (rng.next() % 2048) as usize + 8;
        let mut b = rng.bytes(n);
        if rng.next() & 1 == 0 && b.len() >= 4 {
            b[0..4].copy_from_slice(b"FILE");
        }
        let _ = MftRecord::parse(&b, 1024);
    }
}

#[test]
fn usn_and_indx_parsers_never_panic() {
    let mut rng = Rng(0x0F0F_0F0F_1111_2222);
    for _ in 0..10_000 {
        let n = (rng.next() % 8192) as usize;
        let b = rng.bytes(n);
        let _ = usn::parse_usn_stream(&b);
        let _ = usn::scan_indx_names(&b);
    }
    // Seeded INDX blocks.
    for _ in 0..2000 {
        let mut b = rng.bytes(8192);
        b[0..4].copy_from_slice(b"INDX");
        let _ = usn::scan_indx_names(&b);
    }
}

#[test]
fn ntfs_volume_never_panics() {
    let mut rng = Rng(0x7777_8888_9999_AAAA);
    for _ in 0..2000 {
        let n = (rng.next() % 65536) as usize + 16;
        run_volume(rng.bytes(n));
    }
    // Seeded: NTFS boot with plausible geometry, random rest.
    for _ in 0..2000 {
        let mut b = rng.bytes(256 * 1024);
        b[3..11].copy_from_slice(b"NTFS    ");
        b[11] = 0x00;
        b[12] = 0x02; // bytes/sector = 512
        b[13] = 8; // sectors/cluster
        b[0x28..0x30].copy_from_slice(&64u64.to_le_bytes()); // total sectors
        b[0x30..0x38].copy_from_slice(&4u64.to_le_bytes()); // MFT cluster
        b[0x40] = (256u16 - 10) as u8; // -10 → 1024-byte records
        b[510] = 0x55;
        b[511] = 0xAA;
        run_volume(b);
    }
}
