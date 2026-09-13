//! `list_sources` and `probe` (doc 08 §5). Enumeration comes from
//! `reclaim-platform-macos`; probing mirrors the CLI `info` command's logic
//! (geometry + a ~1 s read-speed probe + health verdict + partition map).

use crate::ffitypes::{HealthLevel, PartitionInfo, SourceProbe, SourceSummary};
use crate::resolve::Resolved;
use crate::RcError;
use reclaim_block::BlockSource;
use std::time::{Duration, Instant};

/// Enumerate devices, partitions and volumes (doc 08 §1 sidebar).
#[uniffi::export]
pub fn list_sources() -> Result<Vec<SourceSummary>, RcError> {
    let disks = reclaim_platform_macos::enumerate()
        .map_err(|e| RcError::internal(format!("enumerate: {e}")))?;
    Ok(disks.iter().map(SourceSummary::from).collect())
}

/// Probe one source: geometry, read speed, health, partition map (doc 08 §2.2).
#[uniffi::export]
pub fn probe(source: String) -> Result<SourceProbe, RcError> {
    let resolved = Resolved::parse(&source)?;
    let container = match &resolved {
        Resolved::Image { path } => {
            let c = reclaim_block::detect_container(path);
            if c != reclaim_block::Container::Raw {
                Some(c.label().to_string())
            } else {
                None
            }
        }
        _ => None,
    };
    let src = resolved.open()?;

    let probe = read_speed_probe(src.as_ref(), Duration::from_secs(1), 8 * 1024 * 1024);
    let smart = match &resolved {
        Resolved::Device { disk: Some(d), .. } if d.whole && !d.is_apfs_container() => d.smart,
        _ => reclaim_platform_macos::SmartSummary::NotSupported,
    };
    const SLOW_BPS: f64 = 5.0 * 1024.0 * 1024.0;
    let verdict = reclaim_platform_macos::assess_health(
        smart,
        probe.bad_sectors,
        Some(probe.bytes_per_sec()),
        SLOW_BPS,
    );

    let mut warnings: Vec<String> = Vec::new();
    if verdict.image_first() {
        warnings.push(format!(
            "drive health is {} — image first, then scan the image (docs/plan/06 §7).",
            verdict.label()
        ));
    }
    if let Resolved::Device { disk: Some(d), .. } = &resolved {
        if d.internal == Some(true) {
            warnings.push(
                "internal Apple SSD: TRIM discards freed blocks immediately, so carving deleted \
                 files is mostly futile; rely on APFS snapshots/checkpoints and .Trash."
                    .to_string(),
            );
        }
        if d.apfs.as_ref().and_then(|a| a.filevault) == Some(true) {
            warnings.push(
                "FileVault is enabled; recovery reads the OS-unlocked volume device.".to_string(),
            );
        }
    }

    let map = reclaim_part::scan(&src);
    for note in &map.notes {
        warnings.push(format!("partitions: {note}"));
    }
    let partitions = map
        .entries
        .iter()
        .map(|p| PartitionInfo {
            index: p.index,
            type_label: p.type_label.clone(),
            kind: p
                .type_guid
                .clone()
                .or_else(|| p.type_byte.map(|b| format!("0x{b:02x}")))
                .unwrap_or_default(),
            start: p.start,
            len: p.len,
            name: p.name.clone(),
        })
        .collect();

    Ok(SourceProbe {
        source,
        size: src.len(),
        sector_size_logical: src.sector_size(),
        sector_size_physical: src.physical_sector_size(),
        read_speed_bps: probe.bytes_per_sec(),
        probe_bad_sectors: probe.bad_sectors,
        health: HealthLevel::from(verdict),
        smart: smart.label().to_string(),
        image_first: verdict.image_first(),
        scheme: map.scheme.label().to_string(),
        partitions,
        warnings,
        container,
    })
}

/// A minimal read-speed / read-error probe (mirror of the CLI's `read_speed_probe`).
struct ReadProbe {
    bytes_read: u64,
    elapsed: Duration,
    bad_sectors: u64,
}

impl ReadProbe {
    fn bytes_per_sec(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s > 0.0 {
            self.bytes_read as f64 / s
        } else {
            0.0
        }
    }
}

fn read_speed_probe(src: &dyn BlockSource, budget: Duration, cap: u64) -> ReadProbe {
    let ss = u64::from(src.sector_size().max(1));
    let chunk = (1024 * 1024 / ss * ss).max(ss) as usize;
    let mut buf = vec![0u8; chunk];
    let mut off = 0u64;
    let mut bytes_read = 0u64;
    let mut bad_sectors = 0u64;
    let len = src.len();
    let start = Instant::now();
    while off < len && bytes_read < cap && start.elapsed() < budget {
        let want = ((len - off).min(chunk as u64)) as usize;
        let r = src.read_at(off, &mut buf[..want]);
        bad_sectors += r.bad_count() as u64;
        bytes_read += want as u64;
        off += want as u64;
    }
    ReadProbe {
        bytes_read,
        elapsed: start.elapsed(),
        bad_sectors,
    }
}
