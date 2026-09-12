//! Pure-Rust block decompressors for the image-container sources
//! (docs/plan/04 §5). Every decoder is fallible and returns `None` rather than
//! panicking on hostile input (build guide Part 1.4 rule 3): a container is
//! untrusted on-disk data, so a bad compressed block becomes a run of
//! bad/zero-filled sectors, never a crash.
//!
//! All backends are pure Rust (`flate2` with the `rust_backend`, `bzip2-rs`,
//! `lzma-rs`, `lzfse_rust`), so they add no dynamic library — the Recovery-Mode
//! build's `otool -L` stays limited to system libraries (docs/plan/06 §4).

use std::io::Read;

/// Compression of one container block. Each format parser maps its own type
/// codes onto these.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Uncompressed — copy verbatim.
    Raw,
    /// A run of zeros (sparse hole / DMG zero-fill).
    Zero,
    /// zlib (RFC 1950) stream — DMG `UDZO`, EnCase E01 chunks.
    Zlib,
    /// Raw DEFLATE (RFC 1951), no zlib wrapper.
    Deflate,
    /// bzip2 — DMG `UDBZ`.
    Bzip2,
    /// LZMA (alone or `.xz`) — DMG `ULMO`.
    Lzma,
    /// LZFSE — DMG `ULFO`.
    Lzfse,
}

/// Upper bound on a single decoded block. Container blocks are small (DMG
/// buffers are ~2 MiB, E01 chunks 32 KiB, cluster formats one cluster). Cap the
/// output so a crafted `uncompressed_len` cannot drive an unbounded allocation.
pub const MAX_BLOCK_OUT: usize = 64 * 1024 * 1024;

/// Decompress `src` with `codec`, producing exactly `out_len` bytes on success.
///
/// Returns `None` if `out_len` exceeds [`MAX_BLOCK_OUT`], the stream is corrupt,
/// or the decoded length does not match `out_len` (a truncated block is treated
/// as unreadable rather than silently zero-padded).
#[must_use]
pub fn decompress(codec: Codec, src: &[u8], out_len: usize) -> Option<Vec<u8>> {
    if out_len > MAX_BLOCK_OUT {
        return None;
    }
    let out = match codec {
        Codec::Zero => vec![0u8; out_len],
        Codec::Raw => {
            if src.len() < out_len {
                return None;
            }
            src.get(..out_len)?.to_vec()
        }
        Codec::Zlib => inflate(flate2::read::ZlibDecoder::new(src), out_len)?,
        Codec::Deflate => inflate(flate2::read::DeflateDecoder::new(src), out_len)?,
        Codec::Bzip2 => inflate(bzip2_rs::DecoderReader::new(src), out_len)?,
        Codec::Lzma => lzma_any(src, out_len)?,
        Codec::Lzfse => lzfse(src, out_len)?,
    };
    if out.len() == out_len {
        Some(out)
    } else {
        None
    }
}

/// Read a decoder to completion into a size-capped buffer.
fn inflate<R: Read>(mut r: R, hint: usize) -> Option<Vec<u8>> {
    // Cap the read so a decompression bomb cannot exhaust memory: the caller
    // knows the exact expected length, so anything past it is corruption.
    let cap = hint.min(MAX_BLOCK_OUT).saturating_add(1);
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.len().saturating_add(n) > cap {
                    return None; // more output than expected ⇒ corrupt/bomb
                }
                out.extend_from_slice(buf.get(..n)?);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(out)
}

/// LZMA-alone first (DMG `ULMO` blocks are raw LZMA1 streams); fall back to `.xz`.
fn lzma_any(src: &[u8], hint: usize) -> Option<Vec<u8>> {
    let mut cur = std::io::Cursor::new(src);
    let mut out = Vec::new();
    if lzma_rs::lzma_decompress(&mut cur, &mut out).is_ok() && out.len() <= MAX_BLOCK_OUT {
        return Some(out);
    }
    let mut cur = std::io::Cursor::new(src);
    let mut out = Vec::new();
    if lzma_rs::xz_decompress(&mut cur, &mut out).is_ok() && out.len() <= MAX_BLOCK_OUT {
        return Some(out);
    }
    let _ = hint;
    None
}

/// LZFSE (DMG `ULFO`).
fn lzfse(src: &[u8], hint: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match lzfse_rust::decode_bytes(src, &mut out) {
        Ok(_) if out.len() <= MAX_BLOCK_OUT && (hint == 0 || out.len() == hint) => Some(out),
        Ok(_) => Some(out), // length mismatch handled by the caller's out_len check
        Err(_) => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn zlib_roundtrip() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
        let comp = zlib_compress(&data);
        let got = decompress(Codec::Zlib, &comp, data.len()).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn zero_and_raw() {
        assert_eq!(decompress(Codec::Zero, &[], 16).unwrap(), vec![0u8; 16]);
        assert_eq!(decompress(Codec::Raw, b"abcdef", 4).unwrap(), b"abcd");
        // Raw shorter than requested ⇒ None (never zero-pad silently).
        assert!(decompress(Codec::Raw, b"ab", 4).is_none());
    }

    #[test]
    fn corrupt_zlib_is_none_not_panic() {
        assert!(decompress(Codec::Zlib, b"\x78\x9c\xde\xad\xbe\xef", 100).is_none());
        assert!(decompress(Codec::Bzip2, b"not bzip2 at all", 100).is_none());
        assert!(decompress(Codec::Lzma, b"\x00\x01\x02", 100).is_none());
        assert!(decompress(Codec::Lzfse, b"\x00\x00\x00\x00", 100).is_none());
    }

    #[test]
    fn out_len_cap_rejected() {
        assert!(decompress(Codec::Zero, &[], MAX_BLOCK_OUT + 1).is_none());
    }

    #[test]
    fn wrong_out_len_rejected() {
        let data = vec![7u8; 100];
        let comp = zlib_compress(&data);
        // Ask for a different length than the stream really holds.
        assert!(decompress(Codec::Zlib, &comp, 99).is_none());
    }
}
