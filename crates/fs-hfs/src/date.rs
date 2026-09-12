//! HFS+ timestamp decoding (Apple TN1150: dates are seconds since
//! 1904-01-01 00:00:00). We convert to a Unix timestamp and format `YYYY-MM-DD`.
//! A self-contained civil-date routine avoids a chrono dependency (workspace's
//! tight dependency budget).

/// Seconds between the HFS+ epoch (1904-01-01) and the Unix epoch (1970-01-01).
const HFS_EPOCH_OFFSET: i64 = 2_082_844_800;

/// Format an HFS+ timestamp (`u32` seconds since 1904) as `YYYY-MM-DD`.
#[must_use]
pub fn hfs_date(secs: u32) -> Option<String> {
    if secs == 0 {
        return None;
    }
    date_from_unix(i64::from(secs) - HFS_EPOCH_OFFSET)
}

/// Format a Unix timestamp (seconds) as `YYYY-MM-DD` (Hinnant civil-from-days).
#[must_use]
pub fn date_from_unix(secs: i64) -> Option<String> {
    if secs <= 0 || secs > 4_102_444_800 {
        return None;
    }
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    Some(format!("{year:04}-{m:02}-{d:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hfs_epoch() {
        // 1904-01-01 → None (zero) ; 2026-09-12 in HFS seconds.
        assert_eq!(hfs_date(0), None);
        let hfs = (1_789_171_200i64 + HFS_EPOCH_OFFSET) as u32;
        assert_eq!(hfs_date(hfs).as_deref(), Some("2026-09-12"));
    }
}
