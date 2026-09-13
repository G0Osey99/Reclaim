//! End-to-end ext2/ext4 recovery against real images built with `mke2fs -d`
//! and deleted with `debugfs rm`. Skips (does not fail) when e2fsprogs is
//! unavailable so CI without it still passes; the Phase-4 CI installs it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use reclaim_block::{BlockSource, ImageFile};
use reclaim_fs_core::{EntryState, FileSystem, VecSink, WalkOpts};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

/// Locate an e2fsprogs tool on PATH or the Homebrew keg-only location.
fn tool(name: &str) -> Option<PathBuf> {
    if Command::new(name).arg("-V").output().is_ok() {
        // On PATH (exit status irrelevant; -V prints to stderr).
        return Some(PathBuf::from(name));
    }
    for dir in ["/opt/homebrew/opt/e2fsprogs/sbin", "/usr/sbin", "/sbin"] {
        let p = PathBuf::from(dir).join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn read_file_bytes(
    src: &Arc<dyn BlockSource>,
    engine: &dyn FileSystem,
    e: &reclaim_fs_core::Entry,
) -> Vec<u8> {
    let mut out = Vec::new();
    let exts = engine.read_extents(e).unwrap_or_default();
    for ext in exts {
        let mut buf = vec![0u8; ext.len as usize];
        let _ = src.read_at(ext.offset, &mut buf);
        out.extend_from_slice(&buf);
    }
    out.truncate(e.size as usize);
    out
}

fn build_and_recover(fs_type: &str) {
    let (Some(mke2fs), Some(debugfs)) = (tool("mke2fs"), tool("debugfs")) else {
        eprintln!("skip {fs_type}: e2fsprogs (mke2fs/debugfs) not found");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let srcdir = dir.path().join("src");
    std::fs::create_dir_all(srcdir.join("DCIM")).unwrap();

    // Known files with distinctive content.
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..6 {
        let name = format!("file_{i:03}.txt");
        let content = format!("ext {fs_type} file {i}\n")
            .repeat(120 + i * 7)
            .into_bytes();
        std::fs::write(srcdir.join(&name), &content).unwrap();
        files.push((name, content));
    }
    let jpg = {
        let mut v = vec![0xFFu8, 0xD8, 0xFF, 0xE0];
        v.extend(std::iter::repeat_n(b'J', 6000));
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    };
    std::fs::write(srcdir.join("DCIM/IMG_0001.jpg"), &jpg).unwrap();

    let img = dir.path().join(format!("{fs_type}.img"));
    // 8 MiB image, 1 KiB blocks.
    let status = Command::new(&mke2fs)
        .args(["-q", "-F", "-t", fs_type, "-b", "1024", "-d"])
        .arg(&srcdir)
        .arg(&img)
        .arg("8192")
        .status()
        .expect("run mke2fs");
    assert!(status.success(), "mke2fs {fs_type} failed");

    // Delete two files + the jpg.
    let deleted = ["file_000.txt", "file_001.txt", "DCIM/IMG_0001.jpg"];
    for d in deleted {
        let st = Command::new(&debugfs)
            .arg("-w")
            .arg("-R")
            .arg(format!("rm /{d}"))
            .arg(&img)
            .status()
            .expect("run debugfs");
        assert!(st.success(), "debugfs rm {d} failed");
    }

    // Open with the engine.
    let src: Arc<dyn BlockSource> = Arc::new(ImageFile::open(&img).unwrap());
    let probe = fs_ext::probe(&src).unwrap_or_else(|| panic!("{fs_type}: probe failed"));
    assert_eq!(probe.fs_kind, "ext");
    let engine = fs_ext::open(src.clone(), probe).unwrap();
    let mut sink = VecSink::default();
    engine.walk(&mut sink, &WalkOpts::default()).unwrap();

    // Live files still present.
    for name in ["file_002.txt", "file_003.txt"] {
        assert!(
            sink.entries
                .iter()
                .any(|e| e.name == name && e.state == EntryState::Live),
            "{fs_type}: live {name} missing"
        );
    }

    // Deleted files recovered by name with exact content.
    for name in ["file_000.txt", "file_001.txt"] {
        let orig = &files.iter().find(|(n, _)| n == name).unwrap().1;
        let rec = sink
            .entries
            .iter()
            .find(|e| e.name == name && e.state.is_recoverable_target())
            .unwrap_or_else(|| panic!("{fs_type}: deleted {name} not recovered"));
        let got = read_file_bytes(&src, &engine, rec);
        assert_eq!(&got, orig, "{fs_type}: {name} content mismatch");
    }

    // Deleted JPEG recovered by name + magic.
    let jrec = sink
        .entries
        .iter()
        .find(|e| e.name == "IMG_0001.jpg" && e.state.is_recoverable_target())
        .unwrap_or_else(|| panic!("{fs_type}: deleted jpg not recovered"));
    let jgot = read_file_bytes(&src, &engine, jrec);
    assert_eq!(jgot, jpg, "{fs_type}: jpg content mismatch");

    eprintln!("{fs_type}: recovered deleted files by name + content OK");
}

#[test]
fn ext4_delete_recovery() {
    build_and_recover("ext4");
}

#[test]
fn ext2_delete_recovery() {
    build_and_recover("ext2");
}
