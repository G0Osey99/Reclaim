//! `reclaim list` — enumerate sources (docs/plan/07 §2).

use crate::exit::{CmdError, CmdResult, Exit};
use crate::util::format_size;
use comfy_table::{Cell, ContentArrangement, Table};
use reclaim_platform_macos::{enumerate, Disk, PlatformError};

/// Run `reclaim list`.
pub fn run(json: bool, plain: bool) -> CmdResult {
    let disks = enumerate().map_err(map_err)?;

    if json {
        let text = serde_json::to_string_pretty(&disks)
            .map_err(|e| CmdError::internal(format!("serializing sources: {e}")))?;
        println!("{text}");
    } else {
        print_table(&disks, plain);
    }
    Ok(Exit::Success)
}

fn map_err(e: PlatformError) -> CmdError {
    match e {
        PlatformError::Unsupported => {
            CmdError::internal("device enumeration is only supported on macOS")
        }
        other => CmdError::internal(other.to_string()),
    }
}

fn print_table(disks: &[Disk], plain: bool) {
    let mut table = Table::new();
    if plain {
        table.load_preset(comfy_table::presets::NOTHING);
    } else {
        table.load_preset(comfy_table::presets::UTF8_FULL);
        table.set_content_arrangement(ContentArrangement::Dynamic);
    }
    table.set_header(vec![
        "DISK",
        "SIZE",
        "BUS",
        "MODEL / NAME",
        "CONTENT / FS",
        "MOUNT",
        "HEALTH",
    ]);

    for d in disks {
        let indent = if d.whole { "" } else { "  " };
        let disk_cell = format!("{indent}{}", d.bsd_name);

        let model = if d.is_apfs_container() {
            "(APFS container)".to_string()
        } else if d.whole {
            d.model.clone().unwrap_or_else(|| "—".to_string())
        } else {
            d.volume_name.clone().unwrap_or_else(|| "—".to_string())
        };

        let content = d
            .fs_kind
            .clone()
            .or_else(|| d.content.clone())
            .unwrap_or_else(|| "—".to_string());

        let mount = match &d.mount_point {
            Some(m) => {
                let fv = d
                    .apfs
                    .as_ref()
                    .and_then(|a| a.filevault)
                    .map(|on| if on { "  FileVault: on" } else { "" })
                    .unwrap_or("");
                format!("{m}{fv}")
            }
            None => String::new(),
        };

        let health = if d.whole && !d.is_apfs_container() {
            d.smart.label().to_string()
        } else {
            String::new()
        };

        let bus = d.bus.clone().unwrap_or_default();

        table.add_row(vec![
            Cell::new(disk_cell),
            Cell::new(format_size(d.size)),
            Cell::new(bus),
            Cell::new(model),
            Cell::new(content),
            Cell::new(mount),
            Cell::new(health),
        ]);
    }

    println!("{table}");
}
