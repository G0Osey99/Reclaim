//! `reclaim doctor` — permissions/root/FDA/SIP self-check (docs/plan/07 §2).

use crate::exit::{CmdResult, Exit};
use reclaim_block::{BlockSource, RawDevice};

/// Run `reclaim doctor`. Returns exit 2 if a root/FDA problem is found.
pub fn run(json: bool) -> CmdResult {
    let root = is_root();
    let fda = probe_full_disk_access(root);
    let sip = sip_status();

    if json {
        let v = serde_json::json!({
            "root": root,
            "full_disk_access": fda.as_json(),
            "sip": sip,
        });
        if let Ok(text) = serde_json::to_string_pretty(&v) {
            println!("{text}");
        }
    } else {
        println!("Reclaim doctor — read-only self-check\n");
        report_root(root);
        report_fda(&fda, root);
        report_sip(&sip);
        println!();
        print_fixits(root, &fda);
    }

    let ok = root && matches!(fda, FdaStatus::Granted);
    Ok(if ok { Exit::Success } else { Exit::Permission })
}

fn is_root() -> bool {
    // SAFETY: geteuid is always safe.
    unsafe { libc::geteuid() == 0 }
}

#[derive(Debug)]
enum FdaStatus {
    /// Boot-disk raw read succeeded.
    Granted,
    /// Boot-disk raw read was denied (root but TCC/FDA not granted).
    Denied,
    /// Could not determine (e.g. not root yet).
    Unknown,
}

impl FdaStatus {
    fn as_json(&self) -> &'static str {
        match self {
            FdaStatus::Granted => "granted",
            FdaStatus::Denied => "denied",
            FdaStatus::Unknown => "unknown",
        }
    }
}

/// Attempt a raw read of the boot physical store. Success implies both root and
/// (on macOS 10.15+) Full Disk Access for the terminal app.
fn probe_full_disk_access(root: bool) -> FdaStatus {
    if !root {
        return FdaStatus::Unknown;
    }
    match RawDevice::open("/dev/rdisk0") {
        Ok(dev) => {
            let ss = dev.sector_size().max(512) as usize;
            let mut buf = vec![0u8; ss];
            let rr = dev.read_at(0, &mut buf);
            if rr.all_good() {
                FdaStatus::Granted
            } else {
                FdaStatus::Denied
            }
        }
        Err(_) => FdaStatus::Denied,
    }
}

fn sip_status() -> String {
    match std::process::Command::new("csrutil").arg("status").output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => "unknown (csrutil unavailable)".to_string(),
    }
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "[ ok ]"
    } else {
        "[fail]"
    }
}

fn report_root(root: bool) {
    println!(
        "{} running as root: {}",
        mark(root),
        if root { "yes" } else { "no" }
    );
}

fn report_fda(fda: &FdaStatus, _root: bool) {
    match fda {
        FdaStatus::Granted => println!(
            "{} Full Disk Access: boot-disk raw read succeeded",
            mark(true)
        ),
        FdaStatus::Denied => println!(
            "{} Full Disk Access: boot-disk raw read denied",
            mark(false)
        ),
        FdaStatus::Unknown => {
            println!("[ -- ] Full Disk Access: undetermined (needs root to test)")
        }
    }
}

fn report_sip(sip: &str) {
    // SIP is informational only — Reclaim never requires it disabled.
    println!("[info] SIP: {sip}");
}

fn print_fixits(root: bool, fda: &FdaStatus) {
    if root && matches!(fda, FdaStatus::Granted) {
        println!("All good: you can scan raw devices including the boot disk.");
        return;
    }
    println!("Fix-its:");
    if !root {
        println!("  • Run under sudo:  sudo reclaim <command>");
        println!(
            "    (raw /dev/rdiskN reads require root — the device nodes are root:operator 0640.)"
        );
    }
    if matches!(fda, FdaStatus::Denied) {
        println!("  • Grant Full Disk Access to your terminal app:");
        println!("      System Settings → Privacy & Security → Full Disk Access → enable your terminal, then restart it.");
        println!("    (Since macOS 10.15, a root process still needs FDA to read the raw device behind the boot volume.)");
    }
    println!("  • SIP does not need to be disabled — Reclaim never requires it (docs/plan/06 §2).");
}
