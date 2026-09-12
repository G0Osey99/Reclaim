//! Scan/resume gate (build guide Phase-1 gates): interrupt at ~40%, resume,
//! and prove the result set is identical to an uninterrupted scan.

use reclaim_block::{BlockSource, ImageFile};
use reclaim_carve::crc::crc32;
use reclaim_carve::engine::CarveOptions;
use reclaim_session::{QueryFilter, ScanConfig, Session, SourceInfo};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn png(size: usize) -> Vec<u8> {
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        let mut body = Vec::from(&kind[..]);
        body.extend_from_slice(data);
        v.extend_from_slice(&crc32(&body).to_be_bytes());
        v
    }
    let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]));
    let pad = size.saturating_sub(v.len() + 24);
    v.extend_from_slice(&chunk(b"IDAT", &vec![0x7u8; pad]));
    v.extend_from_slice(&chunk(b"IEND", &[]));
    v
}

fn build_image() -> tempfile::NamedTempFile {
    // 40 MiB with a PNG every 4 MiB (10 PNGs), 4 KiB-aligned.
    let total = 40 * 1024 * 1024usize;
    let mut img = vec![0u8; total];
    for i in 0..10 {
        let off = i * 4 * 1024 * 1024 + 4096;
        let p = png(2000 + i * 100);
        img[off..off + p.len()].copy_from_slice(&p);
    }
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(&img).unwrap();
    f.flush().unwrap();
    f
}

fn source_info(src: &Arc<dyn BlockSource>) -> SourceInfo {
    SourceInfo {
        source_id: src.id().as_str().to_string(),
        size: src.len(),
        sector_size: src.sector_size(),
    }
}

fn all_ids(session: &Session) -> Vec<String> {
    let mut ids: Vec<String> = session
        .store()
        .query(&QueryFilter::default())
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    ids.sort();
    ids
}

#[test]
fn interrupted_scan_resumes_to_identical_result_set() {
    let f = build_image();
    let src: Arc<dyn BlockSource> = Arc::new(ImageFile::open(f.path()).unwrap());
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();

    let opts = CarveOptions {
        block_size: 4096,
        ..Default::default()
    };

    // Reference: one clean scan.
    let mut sa = Session::create(dir_a.path(), source_info(&src)).unwrap();
    let cfg = ScanConfig {
        carve: opts.clone(),
        checkpoint_secs: 1,
        ..Default::default()
    };
    let rep = sa.run_carve(&src, &cfg, &mut |_e| {}).unwrap();
    assert!(!rep.interrupted);
    let ref_ids = all_ids(&sa);
    assert!(
        ref_ids.len() >= 8,
        "expected ~10 PNGs, got {}",
        ref_ids.len()
    );

    // Interrupt at ~40%, then resume.
    let mut sb = Session::create(dir_b.path(), source_info(&src)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_cb = Arc::clone(&cancel);
    let cfg_int = ScanConfig {
        carve: opts.clone(),
        checkpoint_secs: 1,
        emit_events: true,
        cancel: Some(Arc::clone(&cancel)),
        ..Default::default()
    };
    let rep1 = sb
        .run_carve(&src, &cfg_int, &mut |ev| {
            if let reclaim_session::Event::Progress { pct, .. } = ev {
                if *pct >= 40.0 {
                    cancel_cb.store(true, Ordering::SeqCst);
                }
            }
        })
        .unwrap();
    assert!(rep1.interrupted, "scan should have been interrupted");

    // Resume (fixed block size from the stored plan; global cancel).
    let cfg_resume = ScanConfig {
        carve: opts.clone(),
        resume: true,
        checkpoint_secs: 1,
        ..Default::default()
    };
    let rep2 = sb.run_carve(&src, &cfg_resume, &mut |_e| {}).unwrap();
    assert!(!rep2.interrupted);

    let resumed_ids = all_ids(&sb);
    assert_eq!(
        resumed_ids, ref_ids,
        "resumed result set must match the clean scan"
    );
}
