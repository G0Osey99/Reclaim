//! Report generation (docs/plan/02 FR-RES-5): JSON, HTML and CSV summaries of a
//! session's carved results.
#![forbid(unsafe_code)]

use reclaim_session::CarvedRecord;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Output format.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Format {
    /// Machine-readable JSON.
    Json,
    /// Human-readable HTML.
    Html,
    /// Spreadsheet-friendly CSV.
    Csv,
}

impl Format {
    /// Parse a `--format`/extension string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Some(Format::Json),
            "html" | "htm" => Some(Format::Html),
            "csv" => Some(Format::Csv),
            _ => None,
        }
    }
}

/// Report header/summary.
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    /// Source identity.
    pub source_id: String,
    /// Total results.
    pub total: u64,
    /// Count per family.
    pub by_family: BTreeMap<String, u64>,
    /// Count per validity.
    pub by_validity: BTreeMap<String, u64>,
    /// Total recoverable bytes.
    pub total_bytes: u64,
    /// Generation timestamp (ISO).
    pub generated_at: String,
}

impl Summary {
    /// Build a summary from records.
    #[must_use]
    pub fn from_records(source_id: &str, records: &[CarvedRecord], generated_at: String) -> Self {
        let mut by_family = BTreeMap::new();
        let mut by_validity = BTreeMap::new();
        let mut total_bytes = 0u64;
        for r in records {
            *by_family.entry(r.family.clone()).or_insert(0) += 1;
            *by_validity.entry(r.validity.clone()).or_insert(0) += 1;
            total_bytes = total_bytes.saturating_add(r.len);
        }
        Summary {
            source_id: source_id.to_string(),
            total: records.len() as u64,
            by_family,
            by_validity,
            total_bytes,
            generated_at,
        }
    }
}

/// Render a report.
#[must_use]
pub fn render(records: &[CarvedRecord], summary: &Summary, fmt: Format) -> String {
    match fmt {
        Format::Json => render_json(records, summary),
        Format::Csv => render_csv(records),
        Format::Html => render_html(records, summary),
    }
}

#[derive(Serialize)]
struct Row<'a> {
    id: &'a str,
    path: String,
    family: &'a str,
    format: &'a str,
    ext: &'a str,
    offset: u64,
    len: u64,
    validity: &'a str,
    score: u8,
    date: Option<&'a str>,
    model: Option<&'a str>,
}

fn rows(records: &[CarvedRecord]) -> Vec<Row<'_>> {
    records
        .iter()
        .map(|r| Row {
            id: &r.id,
            path: r.synth_path(),
            family: &r.family,
            format: &r.format,
            ext: &r.ext,
            offset: r.offset,
            len: r.len,
            validity: &r.validity,
            score: r.score,
            date: r.date.as_deref(),
            model: r.model.as_deref(),
        })
        .collect()
}

fn render_json(records: &[CarvedRecord], summary: &Summary) -> String {
    #[derive(Serialize)]
    struct Doc<'a> {
        summary: &'a Summary,
        results: Vec<Row<'a>>,
    }
    let doc = Doc {
        summary,
        results: rows(records),
    };
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

fn csv_escape(s: &str) -> String {
    // Neutralize spreadsheet formula injection, then quote per RFC 4180.
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

fn render_csv(records: &[CarvedRecord]) -> String {
    let mut out = String::from("id,path,family,format,ext,offset,len,validity,score,date,model\n");
    for r in rows(records) {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{}",
            csv_escape(r.id),
            csv_escape(&r.path),
            csv_escape(r.family),
            csv_escape(r.format),
            csv_escape(r.ext),
            r.offset,
            r.len,
            csv_escape(r.validity),
            r.score,
            csv_escape(r.date.unwrap_or("")),
            csv_escape(r.model.unwrap_or("")),
        );
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn render_html(records: &[CarvedRecord], summary: &Summary) -> String {
    let mut out = String::new();
    out.push_str(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Reclaim report</title>",
    );
    out.push_str("<style>body{font:14px system-ui,sans-serif;margin:2rem;color:#111}h1{font-size:1.3rem}table{border-collapse:collapse;width:100%;margin-top:1rem}th,td{border-bottom:1px solid #ddd;padding:4px 8px;text-align:left;font-variant-numeric:tabular-nums}th{background:#f4f4f5}code{font-size:12px}.suspect{color:#b45309}.truncated{color:#a16207}</style></head><body>");
    let _ = write!(out, "<h1>Reclaim recovery report</h1><p>Source: <code>{}</code> · {} results · {} bytes · generated {}</p>",
        html_escape(&summary.source_id), summary.total, summary.total_bytes, html_escape(&summary.generated_at));
    out.push_str("<p>");
    for (fam, n) in &summary.by_family {
        let _ = write!(out, "<b>{}</b>: {} &nbsp; ", html_escape(fam), n);
    }
    out.push_str("</p>");
    out.push_str("<table><thead><tr><th>Path</th><th>Format</th><th>Size</th><th>Validity</th><th>Score</th><th>Offset</th></tr></thead><tbody>");
    for r in rows(records) {
        let _ = write!(
            out,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"{}\">{}</td><td>{}</td><td><code>0x{:x}</code></td></tr>",
            html_escape(&r.path),
            html_escape(r.format),
            r.len,
            r.validity,
            r.validity,
            r.score,
            r.offset,
        );
    }
    out.push_str("</tbody></table></body></html>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reclaim_session::CarvedRecord;

    fn rec() -> CarvedRecord {
        CarvedRecord {
            id: "abc123".into(),
            source_id: "src".into(),
            engine: "carve".into(),
            offset: 4096,
            len: 1000,
            format: "image.png".into(),
            family: "image".into(),
            ext: "png".into(),
            validity: "full".into(),
            score: 97,
            block_aligned: true,
            name: None,
            date: Some("2026-01-02".into()),
            model: None,
            thumb_offset: None,
            thumb_len: None,
            path: None,
            state: None,
            kind: "file".into(),
            extents_json: None,
            merged: false,
        }
    }

    #[test]
    fn csv_escape_neutralizes_formulas_and_quotes() {
        assert_eq!(csv_escape("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(csv_escape("+1"), "'+1");
        assert_eq!(csv_escape("-1"), "'-1");
        assert_eq!(csv_escape("@x"), "'@x");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("l1\rl2"), "\"l1\rl2\"");
        assert_eq!(csv_escape("plain"), "plain");
    }

    #[test]
    fn all_formats_render() {
        let recs = vec![rec()];
        let sum = Summary::from_records("src", &recs, "2026-09-12T00:00:00Z".into());
        let j = render(&recs, &sum, Format::Json);
        assert!(j.contains("image.png") && j.contains("\"total\": 1"));
        let c = render(&recs, &sum, Format::Csv);
        assert!(c.lines().count() == 2);
        let h = render(&recs, &sum, Format::Html);
        assert!(h.contains("<table") && h.contains("image.png"));
    }
}
