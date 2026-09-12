//! Small formatting/probe helpers shared by the CLI commands.

use reclaim_block::BlockSource;
use std::time::{Duration, Instant};

/// Format a byte count the way macOS storage UIs do (decimal, 1000-based).
#[must_use]
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let label = UNITS.get(unit).copied().unwrap_or("B");
    if value >= 100.0 {
        format!("{value:.0} {label}")
    } else {
        format!("{value:.1} {label}")
    }
}

/// Result of a short sequential read-speed probe.
#[derive(Debug, Clone, Copy)]
pub struct ProbeResult {
    /// Bytes read during the probe.
    pub bytes_read: u64,
    /// Wall-clock elapsed.
    pub elapsed: Duration,
    /// Sectors reported bad during the probe.
    pub bad_sectors: u64,
}

impl ProbeResult {
    /// Throughput in bytes/second (0 if no time elapsed).
    #[must_use]
    pub fn bytes_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs <= 0.0 {
            0.0
        } else {
            self.bytes_read as f64 / secs
        }
    }

    /// Human throughput string, e.g. `212 MB/s`.
    #[must_use]
    pub fn rate_string(&self) -> String {
        format!("{}/s", format_size(self.bytes_per_sec() as u64))
    }
}

/// Read sequentially from the start of `src` for about `budget`, using
/// `block` bytes per read, and report throughput. Bad sectors are counted, not
/// fatal. Read-only.
#[must_use]
pub fn read_speed_probe(src: &dyn BlockSource, budget: Duration, block: usize) -> ProbeResult {
    let ss = src.sector_size().max(1) as usize;
    // Align the block down to a whole number of sectors.
    let block = (block / ss).max(1) * ss;
    let mut buf = vec![0u8; block];
    let mut offset: u64 = 0;
    let mut bytes_read: u64 = 0;
    let mut bad_sectors: u64 = 0;
    let len = src.len();
    let start = Instant::now();

    while offset < len && start.elapsed() < budget {
        let remaining = len - offset;
        let this = std::cmp::min(remaining, block as u64) as usize;
        let slice = match buf.get_mut(..this) {
            Some(s) => s,
            None => break,
        };
        let rr = src.read_at(offset, slice);
        bad_sectors += rr.bad_count() as u64;
        bytes_read += this as u64;
        offset += this as u64;
    }

    ProbeResult {
        bytes_read,
        elapsed: start.elapsed(),
        bad_sectors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_formatting() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1000), "1.0 KB");
        assert_eq!(format_size(1_500_000), "1.5 MB");
        assert_eq!(format_size(2_001_111_162_880), "2.0 TB");
        assert_eq!(format_size(64_000_000_000), "64.0 GB");
    }
}
