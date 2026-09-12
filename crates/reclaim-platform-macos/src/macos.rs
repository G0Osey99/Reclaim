//! macOS enumeration implementation. See the crate docs for the data-source
//! split (IOKit / getmntinfo / diskutil).

use crate::{ApfsMembership, Disk, PlatformError, SmartSummary};
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation_sys::base::{kCFAllocatorDefault, CFTypeRef};
use core_foundation_sys::dictionary::{
    CFDictionaryGetValue, CFDictionaryRef, CFMutableDictionaryRef,
};
use io_kit_sys::types::{io_iterator_t, io_registry_entry_t};
use io_kit_sys::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::os::raw::{c_char, c_void};

/// The untyped CoreFoundation dictionary (the only form that implements
/// `ConcreteCFType`, so it is what `downcast` yields). Values are read by
/// CFString key with [`dict_get`].
type CfDict = CFDictionary<*const c_void, *const c_void>;

const KEY_BSD_NAME: &str = "BSD Name";
const KEY_SIZE: &str = "Size";
const KEY_PREFERRED_BLOCK_SIZE: &str = "Preferred Block Size";
const KEY_WHOLE: &str = "Whole";
const KEY_LEAF: &str = "Leaf";
const KEY_REMOVABLE: &str = "Removable";
const KEY_EJECTABLE: &str = "Ejectable";
const KEY_CONTENT: &str = "Content";

// IORegistryEntrySearchCFProperty option bits (from IOKitLib.h).
const K_ITER_RECURSIVELY: u32 = 0x0000_0001;
const K_ITER_PARENTS: u32 = 0x0000_0002;

/// Full enumeration entry point.
pub fn enumerate() -> Result<Vec<Disk>, PlatformError> {
    let mut disks = iokit_media()?;

    // Enrich with mount points / filesystem kind from getmntinfo.
    let mounts = mount_table();
    for d in &mut disks {
        if let Some(m) = mounts.get(&d.bsd_name) {
            d.mount_point = Some(m.mount_point.clone());
            d.fs_kind = Some(m.fs_kind.clone());
        }
    }

    // Enrich with APFS topology (container/volume relations, roles, FileVault).
    if let Some(apfs) = apfs_topology() {
        apply_apfs(&mut disks, &apfs);
    }

    // SMART summary per whole physical disk (skip synthesized containers).
    let whole_names: Vec<String> = disks
        .iter()
        .filter(|d| d.whole && !d.is_apfs_container())
        .map(|d| d.bsd_name.clone())
        .collect();
    let mut smart: HashMap<String, SmartSummary> = HashMap::new();
    for name in whole_names {
        smart.insert(name.clone(), diskutil_smart(&name));
    }
    for d in &mut disks {
        if d.whole && !d.is_apfs_container() {
            if let Some(s) = smart.get(&d.bsd_name) {
                d.smart = *s;
            }
        }
    }

    // Stable order: by whole disk, whole-first, then slice order by name length
    // then lexicographically (disk3 < disk3s1 < disk3s5 < disk3s1s1).
    disks.sort_by(|a, b| {
        crate::whole_disk_of(&a.bsd_name)
            .cmp(&crate::whole_disk_of(&b.bsd_name))
            .then(b.whole.cmp(&a.whole))
            .then(a.bsd_name.len().cmp(&b.bsd_name.len()))
            .then(a.bsd_name.cmp(&b.bsd_name))
    });
    Ok(disks)
}

// ---------------------------------------------------------------------------
// IOKit
// ---------------------------------------------------------------------------

fn iokit_media() -> Result<Vec<Disk>, PlatformError> {
    let mut out = Vec::new();
    // SAFETY: standard IOKit matching-iteration dance; the matching dictionary
    // is consumed by IOServiceGetMatchingServices, iterator + objects released.
    unsafe {
        let class = b"IOMedia\0";
        let matching = IOServiceMatching(class.as_ptr() as *const c_char);
        if matching.is_null() {
            return Err(PlatformError::IoKit(
                "IOServiceMatching returned null".into(),
            ));
        }
        let mut iter: io_iterator_t = 0;
        let kr = IOServiceGetMatchingServices(kIOMasterPortDefault, matching, &mut iter);
        if kr != 0 {
            return Err(PlatformError::IoKit(format!(
                "IOServiceGetMatchingServices failed: {kr:#x}"
            )));
        }
        loop {
            let obj = IOIteratorNext(iter);
            if obj == 0 {
                break;
            }
            if let Some(disk) = disk_from_media(obj) {
                out.push(disk);
            }
            IOObjectRelease(obj);
        }
        IOObjectRelease(iter);
    }
    if out.is_empty() {
        return Err(PlatformError::IoKit("no IOMedia objects found".into()));
    }
    Ok(out)
}

/// Build a [`Disk`] from one IOMedia registry entry. Returns None if it has no
/// BSD name (should not happen for IOMedia).
fn disk_from_media(obj: io_registry_entry_t) -> Option<Disk> {
    let props = entry_properties(obj)?;

    let bsd_name = get_string(&props, KEY_BSD_NAME)?;
    let size = get_i64(&props, KEY_SIZE).unwrap_or(0).max(0) as u64;
    let logical = get_i64(&props, KEY_PREFERRED_BLOCK_SIZE)
        .filter(|v| *v > 0)
        .unwrap_or(512) as u32;
    let whole = get_bool(&props, KEY_WHOLE).unwrap_or(false);
    let leaf = get_bool(&props, KEY_LEAF).unwrap_or(false);
    let removable = get_bool(&props, KEY_REMOVABLE).unwrap_or(false);
    let ejectable = get_bool(&props, KEY_EJECTABLE).unwrap_or(false);
    let content = get_string(&props, KEY_CONTENT).filter(|s| !s.is_empty());

    // Hardware identity + bus from parent characteristics dictionaries.
    let (model, serial) = match search_parent_dict(obj, "Device Characteristics") {
        Some(dc) => (
            get_string(&dc, "Product Name").map(|s| s.trim().to_string()),
            get_string(&dc, "Serial Number").map(|s| s.trim().to_string()),
        ),
        None => (None, None),
    };
    let (bus, internal) = match search_parent_dict(obj, "Protocol Characteristics") {
        Some(pc) => (
            get_string(&pc, "Physical Interconnect"),
            get_string(&pc, "Physical Interconnect Location")
                .map(|loc| loc.eq_ignore_ascii_case("Internal")),
        ),
        None => (None, None),
    };

    Some(Disk {
        whole_disk: crate::whole_disk_of(&bsd_name),
        device_node: format!("/dev/{bsd_name}"),
        raw_device_node: format!("/dev/r{bsd_name}"),
        size,
        logical_block_size: logical,
        physical_block_size: logical,
        whole,
        leaf,
        removable,
        ejectable,
        internal,
        bus,
        model: model.filter(|s| !s.is_empty()),
        serial: serial.filter(|s| !s.is_empty()),
        content,
        mount_point: None,
        fs_kind: None,
        volume_name: None,
        apfs: None,
        smart: SmartSummary::NotAvailable,
        bsd_name,
    })
}

/// Read an entry's property dictionary (untyped CF dictionary).
fn entry_properties(obj: io_registry_entry_t) -> Option<CfDict> {
    let mut props_ref: CFMutableDictionaryRef = std::ptr::null_mut();
    // SAFETY: props_ref is written by the call; we own the returned dictionary.
    let kr =
        unsafe { IORegistryEntryCreateCFProperties(obj, &mut props_ref, kCFAllocatorDefault, 0) };
    if kr != 0 || props_ref.is_null() {
        return None;
    }
    // SAFETY: create-rule: we own props_ref and hand ownership to the wrapper.
    Some(unsafe { CfDict::wrap_under_create_rule(props_ref as CFDictionaryRef) })
}

/// Search an entry's parents (recursively) for a dictionary-valued property.
fn search_parent_dict(obj: io_registry_entry_t, key: &str) -> Option<CfDict> {
    let cfkey = CFString::new(key);
    let plane = b"IOService\0";
    // SAFETY: standard registry property search; create-rule return value.
    let raw = unsafe {
        IORegistryEntrySearchCFProperty(
            obj,
            plane.as_ptr() as *const c_char,
            cfkey.as_concrete_TypeRef(),
            kCFAllocatorDefault,
            K_ITER_RECURSIVELY | K_ITER_PARENTS,
        )
    };
    if raw.is_null() {
        return None;
    }
    // SAFETY: create-rule ownership of the returned CFType.
    let ty = unsafe { CFType::wrap_under_create_rule(raw) };
    ty.downcast_into::<CfDict>()
}

/// Look up a CFString key in an untyped dictionary, returning a typed CFType.
fn dict_get(d: &CfDict, key: &str) -> Option<CFType> {
    let cfkey = CFString::new(key);
    // SAFETY: d and cfkey are valid; CFDictionaryGetValue borrows (get-rule).
    let vref = unsafe {
        CFDictionaryGetValue(
            d.as_concrete_TypeRef(),
            cfkey.as_concrete_TypeRef() as *const c_void,
        )
    };
    if vref.is_null() {
        return None;
    }
    // SAFETY: get-rule — the value is owned by the dictionary.
    Some(unsafe { CFType::wrap_under_get_rule(vref as CFTypeRef) })
}

fn get_string(d: &CfDict, key: &str) -> Option<String> {
    dict_get(d, key)
        .and_then(|v| v.downcast::<CFString>())
        .map(|s| s.to_string())
}

fn get_i64(d: &CfDict, key: &str) -> Option<i64> {
    dict_get(d, key)
        .and_then(|v| v.downcast::<CFNumber>())
        .and_then(|n| n.to_i64())
}

fn get_bool(d: &CfDict, key: &str) -> Option<bool> {
    dict_get(d, key)
        .and_then(|v| v.downcast::<CFBoolean>())
        .map(|b| {
            // SAFETY: b is a valid CFBoolean.
            unsafe { core_foundation_sys::number::CFBooleanGetValue(b.as_concrete_TypeRef()) }
        })
}

// ---------------------------------------------------------------------------
// getmntinfo — mount points and filesystem kinds
// ---------------------------------------------------------------------------

struct MountInfo {
    mount_point: String,
    fs_kind: String,
}

fn mount_table() -> HashMap<String, MountInfo> {
    let mut map = HashMap::new();
    // SAFETY: getmntinfo returns a pointer to a statically-allocated array of
    // `count` statfs structs owned by libc (must not be freed).
    unsafe {
        let mut buf: *mut libc::statfs = std::ptr::null_mut();
        let count = libc::getmntinfo(&mut buf, libc::MNT_NOWAIT);
        if count <= 0 || buf.is_null() {
            return map;
        }
        for i in 0..count as isize {
            let entry = &*buf.offset(i);
            let from = cstr_field(&entry.f_mntfromname);
            let on = cstr_field(&entry.f_mntonname);
            let fs = cstr_field(&entry.f_fstypename);
            // f_mntfromname like "/dev/disk3s5"; key by the bsd name.
            if let Some(bsd) = from.strip_prefix("/dev/") {
                let bsd = bsd.trim_start_matches('r').to_string();
                map.insert(
                    bsd,
                    MountInfo {
                        mount_point: on,
                        fs_kind: fs,
                    },
                );
            }
        }
    }
    map
}

fn cstr_field(field: &[c_char]) -> String {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// diskutil apfs list -plist — APFS topology
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ApfsList {
    #[serde(rename = "Containers", default)]
    containers: Vec<ApfsContainer>,
}

#[derive(Debug, Deserialize)]
struct ApfsContainer {
    #[serde(rename = "ContainerReference")]
    container_reference: String,
    #[serde(rename = "PhysicalStores", default)]
    physical_stores: Vec<ApfsPhysStore>,
    #[serde(rename = "Volumes", default)]
    volumes: Vec<ApfsVolume>,
}

#[derive(Debug, Deserialize)]
struct ApfsPhysStore {
    #[serde(rename = "DeviceIdentifier")]
    device_identifier: String,
}

#[derive(Debug, Deserialize)]
struct ApfsVolume {
    #[serde(rename = "DeviceIdentifier")]
    device_identifier: String,
    #[serde(rename = "Roles", default)]
    roles: Vec<String>,
    #[serde(rename = "Encryption")]
    encryption: Option<bool>,
    #[serde(rename = "FileVault")]
    filevault: Option<bool>,
    #[serde(rename = "Name")]
    name: Option<String>,
}

fn apfs_topology() -> Option<ApfsList> {
    let out = std::process::Command::new("diskutil")
        .args(["apfs", "list", "-plist"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    plist::from_bytes::<ApfsList>(&out.stdout).ok()
}

fn apply_apfs(disks: &mut [Disk], apfs: &ApfsList) {
    // Index volumes by device id for quick lookup.
    let mut index: HashMap<&str, ApfsMembership> = HashMap::new();
    let mut names: HashMap<&str, String> = HashMap::new();
    for c in &apfs.containers {
        index.insert(
            c.container_reference.as_str(),
            ApfsMembership {
                is_container: true,
                physical_stores: c
                    .physical_stores
                    .iter()
                    .map(|p| p.device_identifier.clone())
                    .collect(),
                ..Default::default()
            },
        );
        for v in &c.volumes {
            index.insert(
                v.device_identifier.as_str(),
                ApfsMembership {
                    is_container: false,
                    container_reference: Some(c.container_reference.clone()),
                    roles: v.roles.clone(),
                    encrypted: v.encryption,
                    filevault: v.filevault,
                    ..Default::default()
                },
            );
            if let Some(n) = &v.name {
                names.insert(v.device_identifier.as_str(), n.clone());
            }
        }
    }
    for d in disks.iter_mut() {
        if let Some(m) = index.get(d.bsd_name.as_str()) {
            d.apfs = Some(m.clone());
        }
        if d.volume_name.is_none() {
            if let Some(n) = names.get(d.bsd_name.as_str()) {
                d.volume_name = Some(n.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// diskutil info -plist — SMART summary
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DiskutilInfo {
    #[serde(rename = "SMARTStatus")]
    smart_status: Option<String>,
}

fn diskutil_smart(bsd: &str) -> SmartSummary {
    let out = match std::process::Command::new("diskutil")
        .args(["info", "-plist", bsd])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return SmartSummary::NotAvailable,
    };
    let info: DiskutilInfo = match plist::from_bytes(&out.stdout) {
        Ok(i) => i,
        Err(_) => return SmartSummary::NotAvailable,
    };
    match info.smart_status.as_deref() {
        Some("Verified") => SmartSummary::Verified,
        Some("Failing") | Some("Failed") => SmartSummary::Failing,
        Some("Not Supported") | Some("Unsupported") => SmartSummary::NotSupported,
        _ => SmartSummary::NotAvailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_this_mac() {
        let disks = enumerate().expect("enumeration should succeed on macOS");
        assert!(!disks.is_empty());
        // The internal boot disk is always present.
        assert!(disks.iter().any(|d| d.bsd_name == "disk0" && d.whole));
        // Every disk has a device node and a derived whole-disk name.
        for d in &disks {
            assert!(d.device_node.starts_with("/dev/"));
            assert!(d.raw_device_node.starts_with("/dev/r"));
            assert!(!d.whole_disk.is_empty());
        }
    }

    #[test]
    fn finds_apfs_container_and_data_volume() {
        let disks = enumerate().expect("enumeration");
        // There should be at least one synthesized APFS container.
        assert!(disks.iter().any(Disk::is_apfs_container));
        // A Data-role volume should be discoverable via APFS topology.
        let has_data = disks.iter().any(|d| {
            d.apfs
                .as_ref()
                .is_some_and(|a| a.roles.iter().any(|r| r.eq_ignore_ascii_case("Data")))
        });
        assert!(has_data, "expected an APFS Data-role volume");
    }
}
