//! Deterministic result ids (build guide Part 3.5): `blake3(source_id ||
//! engine || offset || len)`, so the same input image yields identical ids
//! across runs (NFR-5) and resume is idempotent (INSERT OR IGNORE).

/// Compute the stable id for a carved/metadata result.
#[must_use]
pub fn result_id(source_id: &str, engine: &str, offset: u64, len: u64) -> String {
    let mut h = blake3::Hasher::new();
    h.update(source_id.as_bytes());
    h.update(b"|");
    h.update(engine.as_bytes());
    h.update(b"|");
    h.update(&offset.to_le_bytes());
    h.update(b"|");
    h.update(&len.to_le_bytes());
    // 128-bit prefix is plenty for identity and keeps ids compact.
    h.finalize().to_hex().as_str()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_distinct() {
        let a = result_id("image:/x:100", "carve", 4096, 1000);
        let b = result_id("image:/x:100", "carve", 4096, 1000);
        assert_eq!(a, b);
        assert_ne!(a, result_id("image:/x:100", "carve", 4096, 1001));
        assert_eq!(a.len(), 32);
    }
}
