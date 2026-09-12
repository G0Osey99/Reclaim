//! [`BadBlockMap`] — a serializable record of which LBAs are bad/unread on a
//! source. The imager (Phase 1) writes one alongside an image; scans consult it
//! so results touching bad regions can be marked `Suspect` (docs/plan/03 §2.1,
//! docs/plan/09 §2).

use crate::source::SourceId;
use serde::{Deserialize, Serialize};

/// A half-open run of LBAs `[start, start+count)`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LbaRange {
    /// First LBA in the run.
    pub start: u64,
    /// Number of consecutive LBAs.
    pub count: u64,
}

impl LbaRange {
    /// One past the last LBA.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.count)
    }

    /// True if `lba` is in the run.
    #[must_use]
    pub fn contains(&self, lba: u64) -> bool {
        lba >= self.start && lba < self.end()
    }
}

/// Bad/unread LBA ranges for one source, sorted and coalesced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BadBlockMap {
    /// Identity of the source this map describes, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceId>,
    /// Sector size the LBAs are expressed in.
    pub sector_size: u32,
    /// Bad (unreadable) LBA runs.
    #[serde(default)]
    pub bad: Vec<LbaRange>,
    /// Never-read LBA runs (unimaged regions of a `MappedImage`).
    #[serde(default)]
    pub unread: Vec<LbaRange>,
}

impl BadBlockMap {
    /// Empty map for a source with `sector_size`-byte sectors.
    #[must_use]
    pub fn new(sector_size: u32) -> Self {
        BadBlockMap {
            source: None,
            sector_size,
            bad: Vec::new(),
            unread: Vec::new(),
        }
    }

    /// Attach a source identity (builder style).
    #[must_use]
    pub fn with_source(mut self, id: SourceId) -> Self {
        self.source = Some(id);
        self
    }

    /// Record a single bad LBA.
    pub fn add_bad(&mut self, lba: u64) {
        insert_lba(&mut self.bad, lba);
    }

    /// Record a run of bad LBAs.
    pub fn add_bad_range(&mut self, start: u64, count: u64) {
        if count == 0 {
            return;
        }
        self.bad.push(LbaRange { start, count });
        coalesce(&mut self.bad);
    }

    /// Record a single unread LBA.
    pub fn add_unread(&mut self, lba: u64) {
        insert_lba(&mut self.unread, lba);
    }

    /// True if `lba` is marked bad.
    #[must_use]
    pub fn is_bad(&self, lba: u64) -> bool {
        self.bad.iter().any(|r| r.contains(lba))
    }

    /// True if `lba` is marked unread.
    #[must_use]
    pub fn is_unread(&self, lba: u64) -> bool {
        self.unread.iter().any(|r| r.contains(lba))
    }

    /// Total number of bad LBAs.
    #[must_use]
    pub fn bad_sector_count(&self) -> u64 {
        self.bad.iter().map(|r| r.count).sum()
    }

    /// True if there are no bad or unread ranges.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bad.is_empty() && self.unread.is_empty()
    }

    /// Serialize to pretty JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from JSON.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// Insert one LBA into a sorted, coalesced run list.
fn insert_lba(runs: &mut Vec<LbaRange>, lba: u64) {
    runs.push(LbaRange {
        start: lba,
        count: 1,
    });
    coalesce(runs);
}

/// Sort by start and merge overlapping/adjacent runs.
fn coalesce(runs: &mut Vec<LbaRange>) {
    if runs.len() < 2 {
        return;
    }
    runs.sort_by_key(|r| r.start);
    let mut merged: Vec<LbaRange> = Vec::with_capacity(runs.len());
    for r in runs.iter().copied() {
        match merged.last_mut() {
            Some(last) if r.start <= last.end() => {
                let new_end = last.end().max(r.end());
                last.count = new_end - last.start;
            }
            _ => merged.push(r),
        }
    }
    *runs = merged;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_adjacent_and_overlapping() {
        let mut m = BadBlockMap::new(512);
        m.add_bad(5);
        m.add_bad(6);
        m.add_bad(7);
        m.add_bad_range(20, 3);
        m.add_bad_range(22, 5); // overlaps [20,23) -> [20,27)
        assert_eq!(m.bad.len(), 2);
        assert_eq!(m.bad[0], LbaRange { start: 5, count: 3 });
        assert_eq!(
            m.bad[1],
            LbaRange {
                start: 20,
                count: 7
            }
        );
        assert!(m.is_bad(6));
        assert!(m.is_bad(26));
        assert!(!m.is_bad(27));
        assert_eq!(m.bad_sector_count(), 10);
    }

    #[test]
    fn json_round_trip() {
        let mut m = BadBlockMap::new(4096).with_source(SourceId::for_device("disk9", 1234));
        m.add_bad_range(100, 4);
        m.add_unread(7);
        let json = m.to_json().unwrap();
        let back = BadBlockMap::from_json(&json).unwrap();
        assert_eq!(m, back);
    }
}
