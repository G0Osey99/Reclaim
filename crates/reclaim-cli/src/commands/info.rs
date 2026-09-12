//! `reclaim info <SOURCE>` — scheme/size/sector sizes/mounts/health plus a
//! ~1 second read-speed probe (docs/plan/07 §2).

use crate::exit::{CmdError, CmdResult, Exit};
use crate::source::Resolved;
use crate::util::{format_size, read_speed_probe};
use reclaim_block::BlockSource;
use reclaim_platform_macos::{enumerate, Disk};
use std::time::Duration;

/// Run `reclaim info`.
pub fn run(spec: &str, json: bool) -> CmdResult {
    let resolved = Resolved::parse(spec)?;

    // Try to open the source for the probe + true physical sector size. A
    // permission failure on a device is reported but we still print identity.
    let opened = resolved.open();

    let mut lines: Vec<(String, String)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut probe_json = serde_json::Value::Null;
    let mut partition_json = serde_json::Value::Null;

    match &resolved {
        Resolved::Device {
            bsd,
            raw_node,
            disk,
        } => {
            lines.push(("source".into(), format!("{bsd} (raw {raw_node})")));
            if let Some(d) = disk.as_deref() {
                describe_device(d, &mut lines, &mut warnings);
            } else {
                lines.push(("note".into(), "device not found in inventory".into()));
            }
        }
        Resolved::Image { path } => {
            lines.push(("source".into(), format!("image {}", path.display())));
        }
    }

    // Probe + geometry from the opened handle.
    match opened {
        Ok(src) => {
            lines.push((
                "size".into(),
                format!("{} ({} bytes)", format_size(src.len()), src.len()),
            ));
            lines.push((
                "sector size".into(),
                format!(
                    "logical {}, physical {}",
                    src.sector_size(),
                    src.physical_sector_size()
                ),
            ));
            let probe = read_speed_probe(src.as_ref(), Duration::from_secs(1), 8 * 1024 * 1024);
            lines.push((
                "read speed".into(),
                format!(
                    "{} ({} in {:.2}s, {} bad sectors)",
                    probe.rate_string(),
                    format_size(probe.bytes_read),
                    probe.elapsed.as_secs_f64(),
                    probe.bad_sectors
                ),
            ));
            probe_json = serde_json::json!({
                "bytes_read": probe.bytes_read,
                "elapsed_secs": probe.elapsed.as_secs_f64(),
                "bytes_per_sec": probe.bytes_per_sec(),
                "bad_sectors": probe.bad_sectors,
            });

            // Partition scheme (reclaim-part).
            let map = reclaim_part::scan(&src);
            lines.push((
                "partitions".into(),
                format!(
                    "{} ({} entr{}, confidence {:.2})",
                    map.scheme.label(),
                    map.entries.len(),
                    if map.entries.len() == 1 { "y" } else { "ies" },
                    map.confidence
                ),
            ));
            for p in &map.entries {
                let kind = p.type_guid.clone().unwrap_or_else(|| {
                    p.type_byte
                        .map(|b| format!("0x{b:02x}"))
                        .unwrap_or_default()
                });
                lines.push((
                    format!("  part {}", p.index),
                    format!(
                        "{} [{}] @ 0x{:x} ({}){}",
                        p.type_label,
                        kind,
                        p.start,
                        format_size(p.len),
                        p.name
                            .as_deref()
                            .map(|n| format!(" \"{n}\""))
                            .unwrap_or_default()
                    ),
                ));
            }
            for c in &map.containers {
                lines.push((
                    "  container".into(),
                    format!("{} @ 0x{:x} ({})", c.kind, c.offset, c.evidence),
                ));
            }
            for note in &map.notes {
                warnings.push(format!("partitions: {note}"));
            }
            partition_json = serde_json::to_value(&map).unwrap_or(serde_json::Value::Null);
        }
        Err(e) => {
            // Identity already printed; note the probe was skipped.
            if json {
                // fall through; emit what we have plus the error
            }
            warnings.push(format!("read-speed probe skipped: {}", e.message));
            if e.code == Exit::Permission {
                // Emit and exit with the permission code after printing.
                emit(
                    json,
                    spec,
                    &resolved,
                    &lines,
                    &warnings,
                    &probe_json,
                    &partition_json,
                )?;
                return Err(e);
            }
        }
    }

    lines.push((
        "scan plan".into(),
        "engines: carve (Phase 1) + exFAT/FAT/NTFS metadata (Phase 2); APFS/HFS+ → Phase 3; read-only".into(),
    ));

    emit(
        json,
        spec,
        &resolved,
        &lines,
        &warnings,
        &probe_json,
        &partition_json,
    )?;
    Ok(Exit::Success)
}

fn describe_device(d: &Disk, lines: &mut Vec<(String, String)>, warnings: &mut Vec<String>) {
    if let Some(model) = &d.model {
        let serial = d.serial.as_deref().unwrap_or("—");
        lines.push(("model".into(), format!("{model} (serial {serial})")));
    }
    if let Some(bus) = &d.bus {
        let loc = match d.internal {
            Some(true) => " (internal)",
            Some(false) => " (external)",
            None => "",
        };
        lines.push(("bus".into(), format!("{bus}{loc}")));
    }
    if let Some(content) = &d.content {
        lines.push(("content".into(), content.clone()));
    }
    if let (Some(mount), Some(fs)) = (&d.mount_point, &d.fs_kind) {
        lines.push(("mounted".into(), format!("{fs} at {mount}")));
    } else if let Some(fs) = &d.fs_kind {
        lines.push(("filesystem".into(), fs.clone()));
    }
    if let Some(apfs) = &d.apfs {
        if apfs.is_container {
            lines.push((
                "apfs".into(),
                format!("container, stores: {}", apfs.physical_stores.join(", ")),
            ));
        } else if let Some(cref) = &apfs.container_reference {
            let roles = if apfs.roles.is_empty() {
                "—".to_string()
            } else {
                apfs.roles.join(",")
            };
            lines.push(("apfs".into(), format!("volume in {cref}, roles [{roles}]")));
        }
        if apfs.filevault == Some(true) {
            warnings.push("FileVault is enabled; recovery reads the OS-unlocked volume device (docs/plan/06 §5).".into());
        }
    }
    if d.whole && !d.is_apfs_container() {
        lines.push(("health".into(), d.smart.label().to_string()));
    }

    // Boot-disk / TRIM warnings (docs/plan/06 §2, §3).
    if is_boot_disk(&d.whole_disk) {
        warnings.push("this is (part of) the boot disk — stop using this Mac and recover to an external drive (docs/plan/06 §2).".into());
    }
    if d.internal == Some(true) {
        warnings.push("internal Apple SSD: TRIM discards free blocks immediately, so carving deleted files is mostly futile; rely on APFS snapshots/checkpoints and .Trash (docs/plan/06 §3).".into());
    }
}

/// True if `whole` is the whole disk that backs the running system's Data volume.
fn is_boot_disk(whole: &str) -> bool {
    let Ok(disks) = enumerate() else {
        return false;
    };
    disks.iter().any(|d| {
        matches!(
            d.mount_point.as_deref(),
            Some("/System/Volumes/Data") | Some("/")
        ) && d.whole_disk == whole
    })
}

fn emit(
    json: bool,
    spec: &str,
    resolved: &Resolved,
    lines: &[(String, String)],
    warnings: &[String],
    probe: &serde_json::Value,
    partitions: &serde_json::Value,
) -> Result<(), CmdError> {
    if json {
        let mut map = serde_json::Map::new();
        map.insert("source".into(), serde_json::Value::String(spec.to_string()));
        for (k, v) in lines {
            map.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        map.insert(
            "warnings".into(),
            serde_json::Value::Array(
                warnings
                    .iter()
                    .map(|w| serde_json::Value::String(w.clone()))
                    .collect(),
            ),
        );
        if !probe.is_null() {
            map.insert("probe".into(), probe.clone());
        }
        if !partitions.is_null() {
            map.insert("partition_map".into(), partitions.clone());
        }
        if let Resolved::Device { disk: Some(d), .. } = resolved {
            if let Ok(v) = serde_json::to_value(d.as_ref()) {
                map.insert("device".into(), v);
            }
        }
        let text = serde_json::to_string_pretty(&serde_json::Value::Object(map))
            .map_err(|e| CmdError::internal(format!("serializing info: {e}")))?;
        println!("{text}");
    } else {
        for (k, v) in lines {
            println!("{k:>14}: {v}");
        }
        for w in warnings {
            println!("{:>14}: {w}", "warning");
        }
    }
    Ok(())
}
