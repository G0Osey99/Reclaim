//! Round-trip every image container (docs/plan/04 §5) against real files made
//! by `qemu-img` and `hdiutil`: convert a patterned raw image into each format,
//! open it through [`reclaim_block::open_image_auto`], and assert the decoded
//! bytes match the original exactly. Each case is skipped (not failed) when its
//! generator tool is unavailable, so CI on a runner without `qemu-img`/`hdiutil`
//! still passes; the Phase-4 CI installs `qemu-img` (build guide Part 4 gate).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use reclaim_block::BlockSource;
use std::path::Path;
use std::process::Command;

const SIZE: usize = 8 * 1024 * 1024;

/// Deterministic pattern with two 1 MiB all-zero holes (exercise sparse maps).
fn pattern() -> Vec<u8> {
    let mut b = vec![0u8; SIZE];
    for (i, byte) in b.iter_mut().enumerate() {
        let mb = i / (1024 * 1024);
        if mb == 2 || mb == 5 {
            continue;
        }
        *byte = ((i as u64).wrapping_mul(2_654_435_761) >> 13) as u8;
    }
    b
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Read the source and compare: the first `expect.len()` bytes must match
/// exactly, and any trailing bytes (geometry rounding on VHD/VHDX) must be zero.
fn assert_matches(src: &std::sync::Arc<dyn BlockSource>, expect: &[u8], what: &str) {
    assert!(
        src.len() >= expect.len() as u64,
        "{what}: source {} shorter than expected {}",
        src.len(),
        expect.len()
    );
    let total = src.len() as usize;
    let mut got = vec![0u8; total];
    // Read in 64 KiB chunks to exercise the segment walk + decode cache.
    let step = 64 * 1024;
    let mut off = 0usize;
    while off < total {
        let end = (off + step).min(total);
        let r = src.read_at(off as u64, &mut got[off..end]);
        assert!(r.all_good(), "{what}: bad sectors at offset {off}");
        off = end;
    }
    assert!(&got[..expect.len()] == expect, "{what}: content mismatch");
    assert!(
        got[expect.len()..].iter().all(|b| *b == 0),
        "{what}: trailing padding is not zero"
    );
}

fn qemu_convert(raw: &Path, fmt: &str, out: &Path, subformat: Option<&str>) -> bool {
    let mut cmd = Command::new("qemu-img");
    cmd.arg("convert").arg("-f").arg("raw").arg("-O").arg(fmt);
    if let Some(sf) = subformat {
        cmd.arg("-o").arg(format!("subformat={sf}"));
    }
    cmd.arg(raw).arg(out);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

fn hdiutil_convert(raw: &Path, fmt: &str, out_stem: &Path) -> bool {
    Command::new("hdiutil")
        .arg("convert")
        .arg(raw)
        .arg("-format")
        .arg(fmt)
        .arg("-o")
        .arg(out_stem)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn containers_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let data = pattern();
    let raw = dir.path().join("raw.img");
    std::fs::write(&raw, &data).unwrap();

    let mut ran = 0usize;

    if have("qemu-img") {
        for (fmt, detect) in [
            ("vmdk", "vmdk"),
            ("vdi", "vdi"),
            ("vpc", "vhd"),
            ("vhdx", "vhdx"),
            ("qcow2", "qcow2"),
        ] {
            let out = dir.path().join(format!("disk.{fmt}"));
            if !qemu_convert(&raw, fmt, &out, None) {
                eprintln!("skip {fmt}: qemu-img convert failed");
                continue;
            }
            assert!(
                reclaim_block::detect_container(&out)
                    .label()
                    .contains(detect),
                "{fmt}: wrong container detection ({:?})",
                reclaim_block::detect_container(&out)
            );
            let src =
                reclaim_block::open_image_auto(&out).unwrap_or_else(|e| panic!("open {fmt}: {e}"));
            assert_matches(&src, &data, fmt);
            ran += 1;
        }

        // Flat (descriptor + -flat extent) VMDK.
        let flat = dir.path().join("flat.vmdk");
        if qemu_convert(&raw, "vmdk", &flat, Some("monolithicFlat")) {
            let src = reclaim_block::open_image_auto(&flat).expect("open flat vmdk");
            assert_matches(&src, &data, "vmdk-flat");
            ran += 1;
        }
    } else {
        eprintln!("skip qemu-img container cases: tool not installed");
    }

    if have("hdiutil") {
        // UDRO=raw, UDZO=zlib, UDBZ=bzip2, ULFO=lzfse, ULMO=lzma — one per codec.
        for fmt in ["UDRO", "UDZO", "UDBZ", "ULFO", "ULMO"] {
            let stem = dir.path().join(format!("dmg_{fmt}"));
            if !hdiutil_convert(&raw, fmt, &stem) {
                eprintln!("skip dmg {fmt}: hdiutil convert failed");
                continue;
            }
            let dmg = dir.path().join(format!("dmg_{fmt}.dmg"));
            assert_eq!(
                reclaim_block::detect_container(&dmg),
                reclaim_block::Container::Dmg,
                "dmg {fmt}: not detected as DMG"
            );
            let src = reclaim_block::open_image_auto(&dmg).expect("open dmg");
            assert_matches(&src, &data, &format!("dmg {fmt}"));
            ran += 1;
        }
    } else {
        eprintln!("skip dmg cases: hdiutil not available");
    }

    // Split set: always runnable (no external tool).
    {
        let a = dir.path().join("split.001");
        let b = dir.path().join("split.002");
        let half = SIZE / 2;
        std::fs::write(&a, &data[..half]).unwrap();
        std::fs::write(&b, &data[half..]).unwrap();
        assert_eq!(
            reclaim_block::detect_container(&a),
            reclaim_block::Container::Split
        );
        let src = reclaim_block::open_image_auto(&a).expect("open split");
        assert_matches(&src, &data, "split");
        ran += 1;
    }

    eprintln!("containers_roundtrip: {ran} format(s) verified");
    assert!(ran >= 1, "no container case ran");
}
