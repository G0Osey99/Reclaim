//! FFI round-trip tests (build guide Phase-5 prompt D). Drives the same public
//! surface Swift calls, against the exFAT camera golden image, and proves the
//! runtime gate at the layer this session controls: scan a golden image,
//! preview a thumbnail, page results, recover with verify, and refuse a
//! same-disk destination. Image files need no root, so this runs in CI.

use reclaim_ffi::{
    list_sources, open_session, start_scan, version, CollisionPolicy, EventSink, PreviewKind,
    RcError, RecoverOptions, ResultFilter, ScanOptions,
};
use std::sync::{Arc, Mutex};

/// Collects events delivered during a scan/recover (the Swift `EventSink`).
#[derive(Clone)]
struct Collector {
    events: Arc<Mutex<Vec<String>>>,
}
impl Collector {
    fn new() -> Self {
        Collector {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn count_found(&self) -> usize {
        self.events
            .lock()
            .map(|v| v.iter().filter(|s| s.starts_with("found:")).count())
            .unwrap_or(0)
    }
    fn saw_done(&self) -> bool {
        self.events
            .lock()
            .map(|v| v.iter().any(|s| s.starts_with("done:")))
            .unwrap_or(false)
    }
}
impl EventSink for Collector {
    fn on_event(&self, event: reclaim_ffi::RcEvent) {
        use reclaim_ffi::RcEvent as E;
        let line = match event {
            E::Found { path, .. } => format!("found:{path}"),
            E::Done { found, .. } => format!("done:{found}"),
            E::Progress { .. } => "progress".to_string(),
            E::Volume { fs, .. } => format!("volume:{fs}"),
            E::Warning { msg } => format!("warn:{msg}"),
            E::PassComplete { pass } => format!("pass:{pass}"),
            E::ReadError { .. } => "readerror".to_string(),
            E::ImageProgress { .. } => "image".to_string(),
            E::RecoverProgress { path, .. } => format!("recover:{path}"),
        };
        if let Ok(mut v) = self.events.lock() {
            v.push(line);
        }
    }
}

fn golden() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/build/exfat-camera-delete.img")
}

/// Copy the golden into a temp dir so the recover destination shares its
/// filesystem (making the same-disk refusal meaningful).
fn staged_golden(dir: &std::path::Path) -> std::path::PathBuf {
    let dst = dir.join("exfat-camera-delete.img");
    std::fs::copy(golden(), &dst).expect("copy golden");
    dst
}

#[test]
fn version_matches_workspace() {
    assert_eq!(version(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn list_sources_does_not_error() {
    // On macOS this enumerates real disks; off macOS it errors. Either way it
    // must not panic; on this host (macOS) it should return rows.
    let _ = list_sources();
}

#[test]
fn scan_preview_query_recover_and_refuse() {
    if !golden().exists() {
        eprintln!("golden missing; skipping (run scripts/gen-fs-images)");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let img = staged_golden(tmp.path());
    let sess_dir = tmp.path().join("session");
    let sink = Collector::new();

    // --- scan (quick + deep) ---
    let session = start_scan(
        img.to_string_lossy().to_string(),
        Some(sess_dir.to_string_lossy().to_string()),
        ScanOptions::default(),
        Box::new(sink.clone()),
    )
    .expect("start_scan");
    let summary = session.wait().expect("wait").expect("summary");
    assert!(summary.complete, "scan should complete");
    assert!(summary.total_rows > 0, "scan should find results");
    assert!(sink.saw_done(), "a Done event should have been delivered");
    assert!(sink.count_found() > 0, "Found events should have streamed");

    // --- query a page + counts ---
    let page = session
        .query(ResultFilter::default(), 0, 50)
        .expect("query");
    assert!(page.total > 0 && !page.items.is_empty());
    let fams = session.family_counts(false).expect("family_counts");
    assert!(fams.iter().any(|f| f.family == "image"));

    // A second handle on the same dir (proves the GUI can browse the same
    // session files a scan wrote — doc 08 §4 "reads the same session file").
    let browse = open_session(sess_dir.to_string_lossy().to_string()).expect("open_session");
    assert_eq!(browse.count().unwrap(), session.count().unwrap());

    // --- preview a PNG thumbnail (the golden's PNGs are real images) ---
    let images = session
        .query(
            ResultFilter {
                family: Some("image".into()),
                ..Default::default()
            },
            0,
            200,
        )
        .expect("image query");
    // The golden's real PNGs (16×16) decode; its synthetic "JPEG"s and the
    // AppleDouble `._` companions don't. Find the first image whose thumbnail
    // actually renders — a real recovery UI does the same (try, else fall back).
    let mut rendered: Option<reclaim_ffi::ResultRecord> = None;
    for r in &images.items {
        if let Ok(p) = session.preview(r.id.clone(), PreviewKind::Thumbnail, 128) {
            assert_eq!(p.mime, "image/png");
            assert!(
                p.bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
                "thumbnail should be a PNG"
            );
            rendered = Some(r.clone());
            break;
        }
    }
    let recover_target = rendered.expect("at least one image previews as a PNG thumbnail");
    // A hex preview works on any result (byte transform, never fails).
    let hex = session
        .preview(recover_target.id.clone(), PreviewKind::Hex, 0)
        .expect("hex preview");
    assert!(hex.bytes.starts_with(b"00000000  "));

    // --- same-disk refusal (dest shares the golden's temp filesystem) ---
    let dest = tmp.path().join("recovered");
    let refused = session.recover(
        vec![recover_target.id.clone()],
        RecoverOptions {
            dest: dest.to_string_lossy().to_string(),
            preserve_paths: true,
            flat: false,
            collision: CollisionPolicy::Rename,
            verify: true,
            allow_same_device: false,
        },
        Box::new(sink.clone()),
    );
    assert!(
        matches!(refused, Err(RcError::Refused { .. })),
        "same-disk destination must be refused, got {refused:?}"
    );

    // --- recover with verify (override the same-disk guard for the test fs) ---
    let outcome = session
        .recover(
            vec![recover_target.id.clone()],
            RecoverOptions {
                dest: dest.to_string_lossy().to_string(),
                preserve_paths: false,
                flat: true,
                collision: CollisionPolicy::Rename,
                verify: true,
                allow_same_device: true,
            },
            Box::new(sink.clone()),
        )
        .expect("recover");
    assert_eq!(outcome.recovered, 1);
    assert_eq!(outcome.verify_failed, 0);
    assert_eq!(outcome.files.first().and_then(|f| f.verified), Some(true));
    assert!(std::path::Path::new(&outcome.manifest_path).exists());
}

#[test]
fn destination_check_flags_same_disk() {
    if !golden().exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let img = staged_golden(tmp.path());
    let sess_dir = tmp.path().join("session");
    let session = start_scan(
        img.to_string_lossy().to_string(),
        Some(sess_dir.to_string_lossy().to_string()),
        ScanOptions {
            deep: false,
            ..ScanOptions::default()
        },
        Box::new(Collector::new()),
    )
    .expect("start_scan");
    let _ = session.wait();
    let check = session
        .check_destination(tmp.path().join("out").to_string_lossy().to_string(), 1024)
        .expect("check");
    assert!(check.same_disk, "same tempfs must be flagged");
    assert!(!check.ok);
}
