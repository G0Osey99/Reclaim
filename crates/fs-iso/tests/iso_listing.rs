//! List files out of a real ISO 9660 / Joliet image built by `hdiutil
//! makehybrid`, and verify names, paths and exact content via the reported
//! extents. Skipped when `hdiutil` is unavailable (non-macOS CI).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use reclaim_block::{BlockSource, ImageFile};
use reclaim_fs_core::{FileSystem, VecSink, WalkOpts};
use std::process::Command;
use std::sync::Arc;

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn iso9660_joliet_listing() {
    if !have("hdiutil") {
        eprintln!("skip: hdiutil not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("DIR1")).unwrap();
    let readme = b"hello iso world\n".to_vec();
    let photo = b"\xff\xd8\xff\xe0JPEGDATA\xff\xd9".to_vec();
    let nested = b"nested content here\n".repeat(50);
    std::fs::write(src.join("readme.txt"), &readme).unwrap();
    std::fs::write(src.join("photo.jpg"), &photo).unwrap();
    std::fs::write(src.join("DIR1/nested.txt"), &nested).unwrap();

    let iso = dir.path().join("out.iso");
    let ok = Command::new("hdiutil")
        .args([
            "makehybrid",
            "-iso",
            "-joliet",
            "-default-volume-name",
            "RECLAIMISO",
            "-o",
        ])
        .arg(&iso)
        .arg(&src)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skip: hdiutil makehybrid failed");
        return;
    }

    let src: Arc<dyn BlockSource> = Arc::new(ImageFile::open(&iso).unwrap());
    let probe = fs_iso::probe(&src).expect("iso probe");
    assert!(probe.fs_kind.contains("iso") || probe.fs_kind == "joliet");
    let engine = fs_iso::open(src.clone(), probe).unwrap();
    let mut sink = VecSink::default();
    engine.walk(&mut sink, &WalkOpts::default()).unwrap();

    let find = |name: &str| sink.entries.iter().find(|e| e.name == name);
    let content = |name: &str, want: &[u8]| {
        let e = find(name).unwrap_or_else(|| panic!("{name} not listed"));
        let mut got = Vec::new();
        for ext in &e.extents {
            let mut b = vec![0u8; ext.len as usize];
            let _ = src.read_at(ext.offset, &mut b);
            got.extend_from_slice(&b);
        }
        got.truncate(e.size as usize);
        assert_eq!(&got, want, "{name} content mismatch");
    };

    content("readme.txt", &readme);
    content("photo.jpg", &photo);
    // Nested file carries its path.
    let n = find("nested.txt").expect("nested.txt not listed");
    assert_eq!(n.path.as_deref(), Some("DIR1/nested.txt"));
    content("nested.txt", &nested);

    eprintln!("iso listing OK: {} entries", sink.entries.len());
}
