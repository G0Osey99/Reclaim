//! Windows FILETIME → `YYYY-MM-DD` without a calendar dependency.

/// Convert a FILETIME (100-ns intervals since 1601-01-01 UTC) to `YYYY-MM-DD`.
/// Returns `None` for zero or out-of-range values.
#[must_use]
pub fn filetime_date(ft: u64) -> Option<String> {
    if ft == 0 {
        return None;
    }
    // FILETIME epoch (1601) to Unix epoch (1970) offset in seconds.
    const EPOCH_DIFF: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) as i64 - EPOCH_DIFF;
    if !(-62_135_596_800..=253_402_300_799).contains(&secs) {
        return None;
    }
    let days = secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// Howard Hinnant's days→civil algorithm (days since 1970-01-01).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        // 1970-01-01 = FILETIME 116444736000000000.
        assert_eq!(
            filetime_date(116_444_736_000_000_000).as_deref(),
            Some("1970-01-01")
        );
        // 2026-09-12: unix = 1789171200; filetime = (unix + 11644473600)*1e7.
        let ft = (1_789_171_200u64 + 11_644_473_600) * 10_000_000;
        assert_eq!(filetime_date(ft).as_deref(), Some("2026-09-12"));
        assert_eq!(filetime_date(0), None);
    }
}
