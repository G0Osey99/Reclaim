//! GPT GUID decoding and the known partition-type table (docs/plan/04 §1).

/// Decode a 16-byte on-disk GPT GUID to its canonical string form.
///
/// GPT stores the first three groups little-endian and the last two as-is
/// (big-endian byte order), per the UEFI spec. The result is the usual
/// `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX` uppercase form.
#[must_use]
pub fn guid_to_string(raw: &[u8]) -> String {
    if raw.len() < 16 {
        return String::new();
    }
    let g = |i: usize| raw.get(i).copied().unwrap_or(0);
    let d1 = u32::from_le_bytes([g(0), g(1), g(2), g(3)]);
    let d2 = u16::from_le_bytes([g(4), g(5)]);
    let d3 = u16::from_le_bytes([g(6), g(7)]);
    format!(
        "{d1:08X}-{d2:04X}-{d3:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        g(8),
        g(9),
        g(10),
        g(11),
        g(12),
        g(13),
        g(14),
        g(15)
    )
}

/// True if a GUID is all-zero (an unused GPT entry).
#[must_use]
pub fn is_zero_guid(raw: &[u8]) -> bool {
    raw.len() >= 16 && raw.iter().take(16).all(|b| *b == 0)
}

/// Human label for a known GPT partition-type GUID, else `"Unknown"`.
/// Apple type GUIDs are from docs/plan/04 §1.
#[must_use]
pub fn type_label(guid: &str) -> &'static str {
    match guid {
        // Apple (docs/plan/04 §1).
        "7C3457EF-0000-11AA-AA11-00306543ECAC" => "Apple APFS",
        "48465300-0000-11AA-AA11-00306543ECAC" => "Apple HFS+",
        "53746F72-6167-11AA-AA11-00306543ECAC" => "Apple Core Storage",
        "426F6F74-0000-11AA-AA11-00306543ECAC" => "Apple Boot (Recovery)",
        "55465300-0000-11AA-AA11-00306543ECAC" => "Apple UFS",
        "52414944-0000-11AA-AA11-00306543ECAC" => "Apple RAID",
        "52414944-5F4F-11AA-AA11-00306543ECAC" => "Apple RAID offline",
        "4C616265-6C00-11AA-AA11-00306543ECAC" => "Apple Label",
        // Standard / cross-platform.
        "C12A7328-F81F-11D2-BA4B-00A0C93EC93B" => "EFI System",
        "024DEE41-33E7-11D3-9D69-0008C781F39F" => "MBR partition scheme",
        "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7" => "Microsoft Basic Data",
        "E3C9E316-0B5C-4DB8-817D-F92DF00215AE" => "Microsoft Reserved",
        "DE94BBA4-06D1-4D40-A16A-BFD50179D6AC" => "Windows Recovery",
        "5808C8AA-7E8F-42E0-85D2-E1E90434CFB3" => "Windows LDM metadata",
        "AF9B60A0-1431-4F62-BC68-3311714A69AD" => "Windows LDM data",
        "0FC63DAF-8483-4772-8E79-3D69D8477DE4" => "Linux filesystem",
        "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F" => "Linux swap",
        "E6D6D379-F507-44C2-A23C-238F2A3DF928" => "Linux LVM",
        "A19D880F-05FC-4D3B-A006-743F0F84911E" => "Linux RAID",
        "21686148-6449-6E6F-744E-656564454649" => "BIOS boot",
        _ => "Unknown",
    }
}
