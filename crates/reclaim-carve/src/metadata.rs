//! Embedded metadata for naming carved files (docs/plan/05 §6).

use serde::{Deserialize, Serialize};

/// Metadata pulled from a file's own bytes to name it when the filesystem
/// engine gives no name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    /// Embedded original name (ZIP internal name, doc title, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Embedded timestamp as `YYYY-MM-DD` (EXIF DateTimeOriginal, ISOBMFF
    /// creation time, `docProps`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Camera / device model (EXIF `Model`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Absolute offset of an embedded JPEG thumbnail/preview (for `preview`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_offset: Option<u64>,
    /// Length of the embedded thumbnail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_len: Option<u64>,
}

impl Metadata {
    /// True if nothing was extracted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.date.is_none()
            && self.model.is_none()
            && self.thumb_offset.is_none()
    }

    /// Synthesize a recovery path (docs/plan/05 §6):
    /// `{family}/{yyyy-mm-dd}/{model|subtype}/{name|f{offset:016x}}.{ext}`.
    #[must_use]
    pub fn suggested_path(&self, family: &str, subtype: &str, offset: u64, ext: &str) -> String {
        let date = self
            .date
            .as_deref()
            .map_or_else(|| "undated".to_string(), sanitize);
        let bucket = self
            .model
            .as_deref()
            .map(sanitize)
            .unwrap_or_else(|| sanitize(subtype));
        let stem = match &self.name {
            Some(n) => sanitize(n),
            None => format!("f{offset:016x}"),
        };
        let ext = if ext.is_empty() { "bin" } else { ext };
        format!("{family}/{date}/{bucket}/{stem}.{ext}")
    }
}

/// Replace path-hostile characters so a synthesized name is safe on disk.
fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = out.trim().to_string();
    out = if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "unnamed".to_string()
    } else {
        trimmed
    };
    if let Some((idx, _)) = out.char_indices().nth(120) {
        out.truncate(idx);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_with_and_without_metadata() {
        let m = Metadata::default();
        assert_eq!(
            m.suggested_path("image", "jpeg", 0x1234, "jpg"),
            "image/undated/jpeg/f0000000000001234.jpg"
        );
        let m2 = Metadata {
            date: Some("2026-01-02".into()),
            model: Some("Canon EOS R5".into()),
            name: Some("IMG_0001".into()),
            ..Default::default()
        };
        assert_eq!(
            m2.suggested_path("image", "cr3", 0, "cr3"),
            "image/2026-01-02/Canon EOS R5/IMG_0001.cr3"
        );
    }

    #[test]
    fn sanitize_strips_separators() {
        assert_eq!(sanitize("a/b:c"), "a_b_c");
        assert_eq!(sanitize("   "), "unnamed");
        assert_eq!(sanitize(".."), "unnamed");
        assert_eq!(sanitize("."), "unnamed");
    }

    #[test]
    fn sanitize_caps_on_char_boundary() {
        let long: String = std::iter::repeat_n('\u{e9}', 100).collect();
        assert_eq!(long.len(), 200);
        let out = sanitize(&long);
        assert!(out.chars().count() <= 120);
        assert_eq!(out.chars().count(), 100);
        let longer: String = std::iter::repeat_n('\u{e9}', 130).collect();
        assert_eq!(sanitize(&longer).chars().count(), 120);
    }

    #[test]
    fn synth_sanitizes_date() {
        let m = Metadata {
            date: Some("../x".into()),
            ..Default::default()
        };
        assert_eq!(
            m.suggested_path("image", "jpeg", 0, "jpg"),
            "image/.._x/jpeg/f0000000000000000.jpg"
        );
    }
}
