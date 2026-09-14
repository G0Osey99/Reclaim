//! Compile `catalog/*.toml` into a static `SIGNATURES` table.
//!
//! Validates every entry (unique ids, well-formed hex patterns, a non-empty
//! literal anchor per header, valid strategy, footer present when required)
//! and codegens `OUT_DIR/catalog.rs`. A malformed catalog fails the build —
//! the catalog is data, but it is checked data (docs/plan/05 §1).

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    #[serde(default)]
    signature: Vec<RawSig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSig {
    id: String,
    family: String,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    mime: String,
    tier: u8,
    #[serde(default)]
    headers: Vec<RawHeader>,
    #[serde(default)]
    footer: Option<RawFooter>,
    #[serde(default)]
    size: Option<RawSize>,
    strategy: String,
    #[serde(default)]
    validator: Option<String>,
    #[serde(default)]
    metadata: Vec<String>,
    #[serde(default = "default_true")]
    block_aligned: bool,
    #[serde(default)]
    footer_anchored: bool,
    #[serde(default)]
    reassembly: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHeader {
    pattern: String,
    #[serde(default)]
    offset: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFooter {
    pattern: String,
    #[serde(default = "default_search_max")]
    search_max: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSize {
    #[serde(default)]
    min: Option<String>,
    #[serde(default)]
    max: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_search_max() -> String {
    "256MiB".to_string()
}

/// Parse a size string like `1KiB`, `256MiB`, `4GiB`, `512B`, `12MB`, or a bare
/// integer (bytes).
fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mult): (&str, u64) = if let Some(n) = s.strip_suffix("KiB") {
        (n, 1024)
    } else if let Some(n) = s.strip_suffix("MiB") {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("GiB") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("TiB") {
        (n, 1024u64 * 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("KB") {
        (n, 1000)
    } else if let Some(n) = s.strip_suffix("MB") {
        (n, 1_000_000)
    } else if let Some(n) = s.strip_suffix("GB") {
        (n, 1_000_000_000)
    } else if let Some(n) = s.strip_suffix('B') {
        (n, 1)
    } else {
        (s, 1)
    };
    let v: u64 = num.trim().parse().map_err(|_| format!("bad size {s:?}"))?;
    Ok(v.saturating_mul(mult))
}

/// Parse a hex pattern (`FF D8 ?? E1`) into (bytes-with-wildcards-zeroed, mask).
fn parse_pattern(p: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut bytes = Vec::new();
    let mut mask = Vec::new();
    for tok in p.split_whitespace() {
        if tok == "??" {
            bytes.push(0);
            mask.push(0x00);
        } else {
            let b = u8::from_str_radix(tok, 16).map_err(|_| format!("bad hex token {tok:?}"))?;
            bytes.push(b);
            mask.push(0xFF);
        }
    }
    if bytes.is_empty() {
        return Err("empty pattern".to_string());
    }
    Ok((bytes, mask))
}

/// Longest run of literal (mask == 0xFF) bytes; returns (anchor_bytes, start).
fn extract_anchor(bytes: &[u8], mask: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let mut best_start = 0usize;
    let mut best_len = 0usize;
    let mut i = 0usize;
    while i < mask.len() {
        if mask[i] == 0xFF {
            let start = i;
            while i < mask.len() && mask[i] == 0xFF {
                i += 1;
            }
            let len = i - start;
            if len > best_len {
                best_len = len;
                best_start = start;
            }
        } else {
            i += 1;
        }
    }
    if best_len == 0 {
        return Err("pattern has no literal bytes to anchor on".to_string());
    }
    Ok((
        bytes[best_start..best_start + best_len].to_vec(),
        best_start,
    ))
}

fn byte_slice_literal(b: &[u8]) -> String {
    let mut s = String::from("&[");
    for (i, x) in b.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "0x{x:02X}");
    }
    s.push(']');
    s
}

fn str_slice_literal(items: &[String]) -> String {
    let mut s = String::from("&[");
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "{x:?}");
    }
    s.push(']');
    s
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let catalog_dir = manifest.join("catalog");
    println!("cargo:rerun-if-changed=catalog");

    // Deterministic order (NFR-5): collect files sorted, then signatures by id.
    let mut files: Vec<PathBuf> = std::fs::read_dir(&catalog_dir)
        .unwrap_or_else(|e| panic!("read catalog dir {}: {e}", catalog_dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "toml").unwrap_or(false))
        .collect();
    files.sort();

    let mut by_id: BTreeMap<String, RawSig> = BTreeMap::new();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let text =
            std::fs::read_to_string(f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
        let cat: Catalog =
            toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", f.display()));
        for sig in cat.signature {
            if by_id.contains_key(&sig.id) {
                panic!("duplicate signature id {:?} (in {})", sig.id, f.display());
            }
            by_id.insert(sig.id.clone(), sig);
        }
    }

    let mut out = String::new();
    out.push_str("// @generated by reclaim-sigs/build.rs from catalog/*.toml — do not edit.\n");
    out.push_str("pub static SIGNATURES: &[SigDef] = &[\n");

    let mut max_anchor_len = 0usize;
    let mut max_header_span = 0usize;

    for (id, sig) in &by_id {
        let strategy = match sig.strategy.as_str() {
            "fixed" => "Strategy::Fixed",
            "header_size" => "Strategy::HeaderSize",
            "footer" => "Strategy::Footer",
            "structural" => "Strategy::Structural",
            "statistical" => "Strategy::Statistical",
            other => panic!("{id}: unknown strategy {other:?}"),
        };
        if sig.strategy == "footer" && sig.footer.is_none() {
            panic!("{id}: strategy=footer requires a [footer]");
        }
        if sig.family.is_empty() {
            panic!("{id}: empty family");
        }

        // Headers.
        let mut headers_src = String::from("&[");
        for (hi, h) in sig.headers.iter().enumerate() {
            let (bytes, mask) =
                parse_pattern(&h.pattern).unwrap_or_else(|e| panic!("{id} header {hi}: {e}"));
            let (anchor, anchor_pos) =
                extract_anchor(&bytes, &mask).unwrap_or_else(|e| panic!("{id} header {hi}: {e}"));
            max_anchor_len = max_anchor_len.max(anchor.len());
            max_header_span = max_header_span.max(h.offset as usize + bytes.len());
            if hi > 0 {
                headers_src.push_str(", ");
            }
            let _ = write!(
                headers_src,
                "HeaderPat {{ bytes: {}, mask: {}, offset: {}u64, anchor: {}, anchor_pos: {}usize }}",
                byte_slice_literal(&bytes),
                byte_slice_literal(&mask),
                h.offset,
                byte_slice_literal(&anchor),
                anchor_pos,
            );
        }
        headers_src.push(']');
        if sig.headers.is_empty() {
            panic!("{id}: at least one header pattern is required");
        }

        // Footer.
        let footer_src = match &sig.footer {
            Some(ft) => {
                let (fb, _fm) =
                    parse_pattern(&ft.pattern).unwrap_or_else(|e| panic!("{id} footer: {e}"));
                // Footers must be fully literal (used as a plain forward scan).
                if fb.len() != ft.pattern.split_whitespace().count() {
                    panic!("{id}: footer must not contain wildcards");
                }
                let sm = parse_size(&ft.search_max).unwrap_or_else(|e| panic!("{id} footer: {e}"));
                format!(
                    "Some(FooterPat {{ bytes: {}, search_max: {}u64 }})",
                    byte_slice_literal(&fb),
                    sm
                )
            }
            None => "None".to_string(),
        };

        let (min_size, max_size) = match &sig.size {
            Some(sz) => {
                let mn = sz
                    .min
                    .as_deref()
                    .map(|s| parse_size(s).unwrap_or_else(|e| panic!("{id} size.min: {e}")))
                    .unwrap_or(0);
                let mx = sz
                    .max
                    .as_deref()
                    .map(|s| parse_size(s).unwrap_or_else(|e| panic!("{id} size.max: {e}")))
                    .unwrap_or(u64::MAX);
                (mn, mx)
            }
            None => (0u64, u64::MAX),
        };

        let validator_src = match &sig.validator {
            Some(v) => format!("Some({v:?})"),
            None => "None".to_string(),
        };

        let _ = writeln!(
            out,
            "    SigDef {{ id: {id:?}, family: {:?}, extensions: {}, mime: {:?}, tier: {}u8, headers: {}, footer: {}, min_size: {}u64, max_size: {}u64, strategy: {}, validator: {}, metadata: {}, block_aligned: {}, footer_anchored: {}, reassembly: {} }},",
            sig.family,
            str_slice_literal(&sig.extensions),
            sig.mime,
            sig.tier,
            headers_src,
            footer_src,
            min_size,
            max_size,
            strategy,
            validator_src,
            str_slice_literal(&sig.metadata),
            sig.block_aligned,
            sig.footer_anchored,
            sig.reassembly,
        );
    }

    out.push_str("];\n\n");
    let _ = writeln!(
        out,
        "/// Longest Aho-Corasick anchor across the catalog (chunk-overlap size).\npub const MAX_ANCHOR_LEN: usize = {max_anchor_len};"
    );
    let _ = writeln!(
        out,
        "/// Farthest a header pattern reaches from a file start (bytes to confirm).\npub const MAX_HEADER_SPAN: usize = {max_header_span};"
    );

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out_dir.join("catalog.rs"), out).expect("write catalog.rs");
}
