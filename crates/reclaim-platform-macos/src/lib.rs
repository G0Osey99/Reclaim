//! macOS device enumeration for Reclaim (docs/plan/06 §1, docs/plan/03 §7).
//!
//! The device inventory and hardware fields come from **IOKit** (`IOMedia` and
//! the parent `IOBlockStorageDevice` characteristics), which is the same API the
//! Phase 5 privileged helper will use. Mounts and filesystem kind come from
//! `getmntinfo(3)`; APFS container/volume relationships, roles and FileVault
//! state from `diskutil apfs list -plist`; and the SMART summary from
//! `diskutil info -plist` (IOKit SMART passthrough is a Phase 4 item). See the
//! Decisions section of docs/build-log/phase-0.md for the rationale.
//!
//! The public data model compiles on every platform so the CLI can depend on it
//! unconditionally; [`enumerate`] returns [`PlatformError::Unsupported`] off
//! macOS.

use serde::Serialize;

#[cfg(target_os = "macos")]
mod macos;

/// Coarse drive-health summary (docs/plan/06 §7).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartSummary {
    /// SMART reports the device healthy.
    Verified,
    /// SMART reports the device failing — force the image-first path.
    Failing,
    /// The device/bus does not expose SMART (common for USB bridges).
    NotSupported,
    /// SMART status could not be determined.
    NotAvailable,
}

impl SmartSummary {
    /// Short human label for the `list` table HEALTH column.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SmartSummary::Verified => "ok",
            SmartSummary::Failing => "FAILING",
            SmartSummary::NotSupported => "n/a",
            SmartSummary::NotAvailable => "—",
        }
    }
}

/// Overall drive-health verdict (docs/plan/06 §7): combines the IOKit-backed
/// SMART status (via `diskutil`, where the bus exposes it) with a read-error /
/// latency probe (the fallback for USB bridges that pass no SMART).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthVerdict {
    /// Healthy: SMART verified (or absent) and the probe was clean and fast.
    Good,
    /// Marginal: unusually slow, or SMART not verifiable — imaging advised.
    Warn,
    /// Failing: SMART failing or the probe hit read errors — **image first**.
    Bad,
    /// Not enough signal to judge.
    Unknown,
}

impl HealthVerdict {
    /// Short label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            HealthVerdict::Good => "good",
            HealthVerdict::Warn => "marginal",
            HealthVerdict::Bad => "FAILING",
            HealthVerdict::Unknown => "unknown",
        }
    }

    /// True when the planner should steer the user to image the source first
    /// before scanning (a failing/marginal drive should not be hammered).
    #[must_use]
    pub fn image_first(self) -> bool {
        matches!(self, HealthVerdict::Warn | HealthVerdict::Bad)
    }
}

/// Combine a SMART status with read-probe results into a [`HealthVerdict`].
///
/// * any read error in the probe, or SMART `Failing` ⇒ `Bad` (image first);
/// * a successful probe slower than `slow_threshold_bps` ⇒ `Warn`;
/// * SMART `Verified`/absent with a clean, fast probe ⇒ `Good`.
#[must_use]
pub fn assess_health(
    smart: SmartSummary,
    read_errors: u64,
    bytes_per_sec: Option<f64>,
    slow_threshold_bps: f64,
) -> HealthVerdict {
    if smart == SmartSummary::Failing || read_errors > 0 {
        return HealthVerdict::Bad;
    }
    match bytes_per_sec {
        Some(bps) if bps > 0.0 && bps < slow_threshold_bps => HealthVerdict::Warn,
        Some(_) => HealthVerdict::Good,
        None => match smart {
            SmartSummary::Verified => HealthVerdict::Good,
            _ => HealthVerdict::Unknown,
        },
    }
}

/// APFS membership facts for a disk (container or volume).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ApfsMembership {
    /// True if this disk is a synthesized APFS container.
    pub is_container: bool,
    /// For a volume: the container it belongs to (e.g. `disk3`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_reference: Option<String>,
    /// For a container: its physical stores (e.g. `disk0s2`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub physical_stores: Vec<String>,
    /// For a volume: APFS roles (e.g. `["Data"]`, `["System"]`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// For a volume: whether it is encrypted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    /// For a volume: whether FileVault is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filevault: Option<bool>,
}

/// One block device / partition / synthesized volume.
#[derive(Clone, Debug, Serialize)]
pub struct Disk {
    /// BSD identifier, e.g. `disk0`, `disk3s5`.
    pub bsd_name: String,
    /// Buffered device node, e.g. `/dev/disk3s5`.
    pub device_node: String,
    /// Raw (unbuffered) device node used for scanning, e.g. `/dev/rdisk3s5`.
    pub raw_device_node: String,
    /// Parent whole-disk BSD name, e.g. `disk3` for `disk3s5`.
    pub whole_disk: String,
    /// Size in bytes.
    pub size: u64,
    /// Logical (preferred) block size in bytes.
    pub logical_block_size: u32,
    /// Physical block size in bytes (equals logical until refined by opening
    /// the raw device, which `reclaim info` does).
    pub physical_block_size: u32,
    /// True if this is a whole disk (not a partition/slice).
    pub whole: bool,
    /// True if this media has no children (a leaf).
    pub leaf: bool,
    /// True if the media is removable.
    pub removable: bool,
    /// True if the media is ejectable.
    pub ejectable: bool,
    /// True if the device is internal (from Protocol Characteristics).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal: Option<bool>,
    /// Physical interconnect / bus, e.g. `USB`, `PCI-Express`, `Apple Fabric`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus: Option<String>,
    /// Product/model name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Device serial number, when exposed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Content / partition type hint (e.g. `Apple_APFS`, GUID, `GUID_partition_scheme`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Mount point if mounted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_point: Option<String>,
    /// Filesystem kind (e.g. `apfs`, `exfat`, `msdos`, `hfs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fs_kind: Option<String>,
    /// Volume name if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_name: Option<String>,
    /// APFS membership facts, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apfs: Option<ApfsMembership>,
    /// Drive health summary (only meaningful on whole physical disks).
    pub smart: SmartSummary,
}

impl Disk {
    /// True if this disk is a synthesized APFS container.
    #[must_use]
    pub fn is_apfs_container(&self) -> bool {
        self.apfs.as_ref().is_some_and(|a| a.is_container)
    }
}

/// Errors from platform enumeration.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// Enumeration is only implemented on macOS in round 1.
    #[error("device enumeration is only implemented on macOS")]
    Unsupported,
    /// An IOKit call failed.
    #[error("IOKit error: {0}")]
    IoKit(String),
    /// A `diskutil` invocation failed or produced unparseable output.
    #[error("diskutil error: {0}")]
    Diskutil(String),
    /// Underlying I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Enumerate all block devices, partitions and synthesized volumes, sorted by
/// BSD name in a stable device order (whole disks first, then their children).
pub fn enumerate() -> Result<Vec<Disk>, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        macos::enumerate()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(PlatformError::Unsupported)
    }
}

/// Find a single disk by BSD name (e.g. `disk4`, `disk3s5`).
pub fn disk_by_bsd(bsd: &str) -> Result<Option<Disk>, PlatformError> {
    let want = bsd.trim_start_matches("/dev/").trim_start_matches('r');
    Ok(enumerate()?.into_iter().find(|d| d.bsd_name == want))
}

/// Derive the whole-disk BSD name from any BSD name (`disk3s5` -> `disk3`).
#[must_use]
pub fn whole_disk_of(bsd: &str) -> String {
    let name = bsd.trim_start_matches("/dev/").trim_start_matches('r');
    if let Some(rest) = name.strip_prefix("disk") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return format!("disk{digits}");
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_disk_derivation() {
        assert_eq!(whole_disk_of("disk3s5"), "disk3");
        assert_eq!(whole_disk_of("disk3s1s1"), "disk3");
        assert_eq!(whole_disk_of("disk0"), "disk0");
        assert_eq!(whole_disk_of("/dev/rdisk10s2"), "disk10");
        assert_eq!(whole_disk_of("/dev/disk4"), "disk4");
    }

    #[test]
    fn smart_labels() {
        assert_eq!(SmartSummary::Verified.label(), "ok");
        assert_eq!(SmartSummary::Failing.label(), "FAILING");
    }
}
