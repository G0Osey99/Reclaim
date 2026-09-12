#![no_main]
//! Fuzz the APFS byte-level parsers: obj_phys/Fletcher-64, B-tree node
//! (omap + fs-tree), and the j_inode / j_drec / j_file_extent record decoders.
use libfuzzer_sys::fuzz_target;
use fs_apfs::btree::Node;
use fs_apfs::obj::{fletcher64_valid, ObjPhys};
use fs_apfs::records::{parse_dir_rec, parse_file_extent, parse_inode, split_key};

fuzz_target!(|data: &[u8]| {
    let _ = ObjPhys::parse(data);
    let _ = fletcher64_valid(data);
    let _ = Node::parse(data.to_vec());
    let oat = if data.len() >= 8 {
        u64::from_le_bytes([data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7]])
    } else {
        0
    };
    let (obj, _ty) = split_key(oat);
    let _ = parse_inode(obj, data);
    let _ = parse_dir_rec(obj, data, data, true);
    let _ = parse_dir_rec(obj, data, data, false);
    let _ = parse_file_extent(obj, data, data);
});
