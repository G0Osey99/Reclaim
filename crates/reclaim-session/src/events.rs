//! Scan events (docs/plan/03 §2.6) serialized as NDJSON per doc 07 §2.

use serde::Serialize;

/// A scan event. The JSON shapes match docs/plan/07 §2 exactly (`ev` tag).
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "ev", rename_all = "lowercase")]
pub enum Event {
    /// Progress for a pass.
    Progress {
        /// Engine/pass name.
        pass: String,
        /// Current LBA/byte cursor.
        lba: u64,
        /// Percent complete (0–100).
        pct: f64,
        /// Bytes/sec.
        rate: u64,
    },
    /// A result was found.
    Found {
        /// Result id.
        id: String,
        /// Producing engine.
        engine: String,
        /// Synthesized path / name.
        path: String,
        /// Size in bytes.
        size: u64,
        /// Confidence score.
        score: u8,
    },
    /// A read error at an LBA.
    #[serde(rename = "readerror")]
    ReadError {
        /// Offending byte offset.
        lba: u64,
    },
    /// A lost-structure volume was proposed (docs/plan/07 §2).
    Volume {
        /// Volume start byte offset.
        start: u64,
        /// Volume length in bytes.
        len: u64,
        /// Filesystem kind.
        fs: String,
        /// Confidence 0..1.
        confidence: f32,
    },
    /// A non-fatal warning.
    Warning {
        /// Message.
        msg: String,
    },
    /// A pass finished.
    #[serde(rename = "passcomplete")]
    PassComplete {
        /// Engine/pass name.
        pass: String,
    },
    /// The scan finished.
    Done {
        /// Total results found.
        found: u64,
        /// Elapsed seconds.
        elapsed: f64,
        /// True if interrupted (resumable).
        interrupted: bool,
    },
}

impl Event {
    /// Serialize to a single NDJSON line (no trailing newline).
    #[must_use]
    pub fn to_ndjson(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_match_doc07() {
        let e = Event::Progress {
            pass: "carve".into(),
            lba: 100,
            pct: 41.3,
            rate: 2000,
        };
        assert!(e.to_ndjson().contains(r#""ev":"progress""#));
        let f = Event::Found {
            id: "abc".into(),
            engine: "carve".into(),
            path: "image/x.jpg".into(),
            size: 10,
            score: 90,
        };
        let j = f.to_ndjson();
        assert!(j.contains(r#""ev":"found""#));
        assert!(j.contains(r#""engine":"carve""#));
        let d = Event::Done {
            found: 5,
            elapsed: 1.2,
            interrupted: false,
        };
        assert!(d.to_ndjson().contains(r#""ev":"done""#));
    }
}
