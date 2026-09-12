//! `reclaim doctor` — permissions/root/FDA/SIP self-check (docs/plan/07 §2).
//!
//! Raw internal devices sit behind TWO independent gates (verified in Phase 0,
//! docs/plan/06 §12): DAC (the nodes are `root:operator 0640` → need root or the
//! `operator` group) and TCC/Full Disk Access (uid-independent; `sudo` actually
//! *drops* the FDA attribution). Doctor reports both, plus SIP (informational).

use crate::exit::{CmdResult, Exit};
use reclaim_block::{BlockSource, RawDevice};

/// Run `reclaim doctor`. Returns exit 2 if the internal boot device is unreadable.
pub fn run(json: bool) -> CmdResult {
    let root = is_root();
    let operator = in_operator_group();
    let fda = fda_effective();
    let boot = boot_device_access();
    let sip = sip_status();

    if json {
        let v = serde_json::json!({
            "root": root,
            "operator_group": operator,
            "full_disk_access_effective": fda,
            "boot_device_read": boot.as_json(),
            "sip": sip,
        });
        if let Ok(text) = serde_json::to_string_pretty(&v) {
            println!("{text}");
        }
    } else {
        println!("Reclaim doctor — read-only self-check\n");
        println!("{} running as root: {}", mark(root), yn(root));
        println!(
            "{} in 'operator' group (DAC for /dev/rdisk*): {}",
            mark(operator),
            yn(operator)
        );
        println!(
            "{} Full Disk Access effective (TCC): {}",
            mark(fda),
            yn(fda)
        );
        report_boot(&boot);
        println!("[info] SIP: {sip}");
        println!();
        print_fixits(root, operator, fda, &boot);
    }

    Ok(if matches!(boot, BootAccess::Ok) {
        Exit::Success
    } else {
        Exit::Permission
    })
}

fn is_root() -> bool {
    // SAFETY: geteuid is always safe.
    unsafe { libc::geteuid() == 0 }
}

/// Membership in the `operator` group (via `id -Gn`).
fn in_operator_group() -> bool {
    std::process::Command::new("id")
        .arg("-Gn")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .any(|g| g == "operator")
        })
        .unwrap_or(false)
}

/// Can this process read a TCC-protected file? That is the clean test for
/// Full Disk Access, independent of the device-node DAC gate.
fn fda_effective() -> bool {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(format!("{home}/Library/Messages/chat.db"));
        candidates.push(format!("{home}/Library/Safari/History.db"));
    }
    candidates.push("/Library/Application Support/com.apple.TCC/TCC.db".to_string());

    let mut saw_denied = false;
    for path in &candidates {
        match std::fs::File::open(path) {
            Ok(_) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => saw_denied = true,
            Err(_) => {}
        }
    }
    // If everything we tried was absent (not denied), we can't tell — report
    // false conservatively; the boot-device probe still classifies the gate.
    let _ = saw_denied;
    false
}

#[derive(Debug)]
enum BootAccess {
    /// Raw boot-store read succeeded (both gates satisfied).
    Ok,
    /// Denied by TCC (`EPERM`) — FDA not effective for this process (e.g. sudo).
    TccDenied,
    /// Denied by DAC (`EACCES`) — not root and not in the operator group.
    DacDenied,
    /// Some other failure.
    Other(String),
}

impl BootAccess {
    fn as_json(&self) -> &str {
        match self {
            BootAccess::Ok => "ok",
            BootAccess::TccDenied => "tcc_denied",
            BootAccess::DacDenied => "dac_denied",
            BootAccess::Other(_) => "error",
        }
    }
}

/// Probe a raw read of the internal boot disk and classify the outcome.
fn boot_device_access() -> BootAccess {
    match RawDevice::open("/dev/rdisk0") {
        Ok(dev) => {
            let ss = dev.sector_size().max(512) as usize;
            let mut buf = vec![0u8; ss];
            if dev.read_at(0, &mut buf).all_good() {
                BootAccess::Ok
            } else {
                BootAccess::Other("opened but first sector unreadable".into())
            }
        }
        Err(e) => {
            let m = e.to_string();
            if m.contains("Operation not permitted") || m.contains("(os error 1)") {
                BootAccess::TccDenied
            } else if m.contains("Permission denied") || m.contains("(os error 13)") {
                BootAccess::DacDenied
            } else {
                BootAccess::Other(m)
            }
        }
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

fn yn(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn report_boot(b: &BootAccess) {
    match b {
        BootAccess::Ok => println!("{} boot-disk raw read (/dev/rdisk0): OK", mark(true)),
        BootAccess::TccDenied => {
            println!("{} boot-disk raw read: DENIED by TCC (EPERM)", mark(false))
        }
        BootAccess::DacDenied => {
            println!("{} boot-disk raw read: DENIED by DAC (EACCES)", mark(false))
        }
        BootAccess::Other(m) => println!("[fail] boot-disk raw read: {m}"),
    }
}

fn print_fixits(root: bool, operator: bool, fda: bool, boot: &BootAccess) {
    if matches!(boot, BootAccess::Ok) {
        println!("All good: you can raw-read the boot disk (both DAC and TCC gates pass).");
        return;
    }
    println!("Fix-its:");
    match boot {
        BootAccess::TccDenied => {
            println!("  • TCC/Full Disk Access is not effective for this process.");
            if root {
                println!("    You are root, but `sudo` DROPS the FDA attribution. Prefer the");
                println!("    non-sudo path below instead of sudo for internal devices.");
            }
            println!("    Grant Full Disk Access to the app that hosts this shell/terminal,");
            println!("    then fully quit & relaunch it (System Settings → Privacy & Security");
            println!("    → Full Disk Access).");
        }
        BootAccess::DacDenied => {
            if !fda {
                println!("  • Full Disk Access does not appear effective — grant it to your");
                println!("    terminal/host app and relaunch it.");
            }
            if !operator && !root {
                println!("  • DAC: add yourself to the 'operator' group (the /dev/rdisk* nodes");
                println!("    are root:operator), then open a fresh session:");
                println!("        sudo dseditgroup -o edit -a \"$USER\" -t user operator");
                println!("    After that a NON-sudo `reclaim` read works (DAC via operator, TCC");
                println!("    via the granted FDA). This avoids sudo dropping FDA.");
            }
            println!("  • Alternatively run under sudo for NON-TCC-protected media (external");
            println!("    drives, attached images) — sudo is fine there.");
        }
        _ => {
            println!("  • Could not classify the boot-disk read; run");
            println!("    `scripts/platform-proofs.sh` for details.");
        }
    }
    println!("  • SIP does not need to be disabled — Reclaim never requires it (docs/plan/06 §2).");
}
