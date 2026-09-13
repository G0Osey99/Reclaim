#![no_main]
//! Fuzz the image-container parsers (DMG koly/mish, VMDK, VDI, VHD/VHDX, QCOW2,
//! EnCase E01, sparseimage) end-to-end: write the input to a temp file, detect +
//! open it, and read the whole reconstructed image. Any panic is a bug.
use libfuzzer_sys::fuzz_target;
use reclaim_block::BlockSource;
use std::io::Write;

fuzz_target!(|data: &[u8]| {
    let Ok(mut f) = tempfile::NamedTempFile::new() else {
        return;
    };
    if f.write_all(data).is_err() {
        return;
    }
    let _ = f.flush();
    let path = f.path();
    // Detection + parse; unrecognized inputs fall back to raw (Ok(None)).
    if let Ok(Some(src)) = reclaim_block::container::open_container(path) {
        // Read a bounded prefix to exercise the block map + decode cache.
        let len = src.len().min(4 * 1024 * 1024);
        let mut off = 0u64;
        let mut buf = vec![0u8; 64 * 1024];
        while off < len {
            let n = ((len - off) as usize).min(buf.len());
            let _ = src.read_at(off, &mut buf[..n]);
            off += n as u64;
        }
    }
});
