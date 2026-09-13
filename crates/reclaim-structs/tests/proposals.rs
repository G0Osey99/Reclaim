//! Lost-structure proposals against crafted images that mirror the Phase-4
//! golden scenarios (docs/plan/09): a GPT with a wiped primary table, an exFAT
//! volume with a zeroed main boot sector, and an HFS+ volume with a zeroed main
//! volume header. Each must still be located from its surviving backup.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use reclaim_block::{BlockSource, MemorySource};
use std::sync::Arc;

const MIB: usize = 1024 * 1024;

fn src(bytes: Vec<u8>) -> Arc<dyn BlockSource> {
    Arc::new(MemorySource::new(bytes))
}

#[test]
fn gpt_backup_recovers_deleted_partition() {
    let sectors = 8192usize; // 4 MiB, 512-byte LBAs
    let mut img = vec![0u8; sectors * 512];
    let linux_guid: [u8; 16] = [
        0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D,
        0xE4,
    ];
    let first_lba = 40u64;
    let last_lba = 200u64;

    let write_header = |img: &mut [u8], hdr_lba: usize, entries_lba: u64| {
        let o = hdr_lba * 512;
        img[o..o + 8].copy_from_slice(b"EFI PART");
        img[o + 72..o + 80].copy_from_slice(&entries_lba.to_le_bytes());
        img[o + 80..o + 84].copy_from_slice(&4u32.to_le_bytes()); // num entries
        img[o + 84..o + 88].copy_from_slice(&128u32.to_le_bytes()); // entry size
    };
    let write_entry = |img: &mut [u8], entries_lba: usize| {
        let e = entries_lba * 512;
        img[e..e + 16].copy_from_slice(&linux_guid);
        img[e + 32..e + 40].copy_from_slice(&first_lba.to_le_bytes());
        img[e + 40..e + 48].copy_from_slice(&last_lba.to_le_bytes());
    };

    // Primary GPT at LBA 1, entries at LBA 2.
    write_header(&mut img, 1, 2);
    write_entry(&mut img, 2);
    // Backup GPT at the last LBA, entries just before it.
    write_header(&mut img, sectors - 1, (sectors - 2) as u64);
    write_entry(&mut img, sectors - 2);

    // Wipe the primary GPT header + entries (the "deleted partition table").
    for b in img[512..1536].iter_mut() {
        *b = 0;
    }

    let s = src(img);
    let props = reclaim_structs::scan(&s);
    let p = props
        .iter()
        .find(|p| p.start == first_lba * 512 && p.evidence.contains("backup GPT"))
        .expect("backup GPT should recover the partition");
    assert_eq!(p.len, (last_lba - first_lba + 1) * 512);
    assert_eq!(p.fs, "ext"); // Linux type GUID hint
}

#[test]
fn exfat_zeroed_main_boot_found_via_backup() {
    let mut img = vec![0u8; 4 * MIB];
    let v = MIB; // volume start at 1 MiB
    let vol_sectors = 2048u64; // 1 MiB volume (512-byte sectors)

    let mut vbr = vec![0u8; 512];
    vbr[3..11].copy_from_slice(b"EXFAT   ");
    vbr[72..80].copy_from_slice(&vol_sectors.to_le_bytes());
    vbr[108] = 9; // bytes-per-sector shift = 512

    // Backup boot sector lives 12 sectors after the (now zeroed) main VBR.
    let backup_off = v + 12 * 512;
    img[backup_off..backup_off + 512].copy_from_slice(&vbr);
    // Main VBR at `v` is left zeroed.

    let s = src(img);
    let props = reclaim_structs::scan(&s);
    assert!(
        props.iter().any(|p| p.fs == "exfat" && p.start == v as u64),
        "exFAT volume start should be recovered from the backup boot: {props:?}"
    );
}

#[test]
fn hfsplus_zeroed_main_vh_found_via_alternate() {
    let mut img = vec![0u8; 4 * MIB];
    let v = MIB as u64;
    let block_size = 4096u32;
    let total_blocks = 256u32; // 1 MiB volume
    let len = u64::from(block_size) * u64::from(total_blocks);

    let mut vh = vec![0u8; 512];
    vh[0..2].copy_from_slice(b"H+");
    vh[2..4].copy_from_slice(&4u16.to_be_bytes()); // version
    vh[40..44].copy_from_slice(&block_size.to_be_bytes());
    vh[44..48].copy_from_slice(&total_blocks.to_be_bytes());

    // Alternate VH at volume+len-1024 (the main VH at volume+1024 stays zeroed).
    let alt_off = (v + len - 1024) as usize;
    img[alt_off..alt_off + 512].copy_from_slice(&vh);

    let s = src(img);
    let props = reclaim_structs::scan(&s);
    assert!(
        props.iter().any(|p| (p.fs == "hfsplus") && p.start == v),
        "HFS+ volume start should be recovered from the alternate VH: {props:?}"
    );
}

#[test]
fn ext_superblock_proposed() {
    let mut img = vec![0u8; 4 * MIB];
    let v = MIB;
    let sb = v + 1024; // ext superblock is 1 KiB into the volume
    img[sb + 0x18..sb + 0x1C].copy_from_slice(&0u32.to_le_bytes()); // 1 KiB blocks
    img[sb + 0x04..sb + 0x08].copy_from_slice(&256u32.to_le_bytes()); // blocks_count
    img[sb + 0x20..sb + 0x24].copy_from_slice(&8192u32.to_le_bytes()); // blocks/group
    img[sb + 0x38..sb + 0x3A].copy_from_slice(&0xEF53u16.to_le_bytes()); // magic

    let s = src(img);
    let props = reclaim_structs::scan(&s);
    assert!(
        props.iter().any(|p| p.fs == "ext" && p.start == v as u64),
        "ext superblock should be proposed: {props:?}"
    );
}
