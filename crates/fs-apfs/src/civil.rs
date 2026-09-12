//! Minimal UTC date formatting (no chrono dependency, per the workspace's tight
//! dependency budget). Converts a Unix timestamp in seconds to `YYYY-MM-DD`
//! using Howard Hinnant's civil-from-days algorithm.

/// Format a Unix timestamp (seconds since 1970-01-01 UTC) as `YYYY-MM-DD`.
/// Returns `None` for the epoch/zero or implausible values.
#[must_use]
pub fn date_from_unix(secs: i64) -> Option<String> {
    if secs <= 0 || secs > 4_102_444_800 {
        // reject <= epoch and > year 2100 (corrupt).
        return None;
    }
    let days = secs.div_euclid(86_400);
    // civil_from_days (Hinnant): days since 1970-01-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    Some(format!("{year:04}-{m:02}-{d:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        // 2026-09-12 00:00:00 UTC = 1789171200.
        assert_eq!(date_from_unix(1_789_171_200).as_deref(), Some("2026-09-12"));
        // 2000-01-01.
        assert_eq!(date_from_unix(946_684_800).as_deref(), Some("2000-01-01"));
        assert_eq!(date_from_unix(0), None);
    }
}
