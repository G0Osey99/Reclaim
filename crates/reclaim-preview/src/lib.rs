//! In-process preview and thumbnail generation from raw file bytes
//! (docs/plan/08 §2.3, FR-RES-2). Everything here is a **byte → byte** or
//! **byte → string** transform: nothing is written to disk (the read-only
//! guarantee, build guide Part 3.2), and the source device is never touched —
//! callers extract the file's extents (or its embedded-thumbnail extent) first
//! and hand the bytes in.
//!
//! What lives here (Rust):
//! * Raster **thumbnails/previews** for JPEG / PNG / GIF / BMP / WebP via the
//!   `image` crate. HEIC and camera-RAW previews are produced by decoding the
//!   file's *embedded JPEG* (the validators record its offset/length as
//!   `thumb_offset`/`thumb_len`); a full HEIC/RAW decode is done Swift-side with
//!   ImageIO from these same bytes.
//! * **Text** and **hex** previews.
//!
//! What is deliberately *not* here: PDF page 1 and the video first frame are
//! rendered Swift-side (PDFKit / AVFoundation) from a temp extraction, because
//! they need system frameworks. This crate stays pure Rust so it fuzzes and
//! builds into the universal static library with no dylib.
//!
//! # Robustness
//! Carved bytes are frequently corrupt (NFR-6). Every entry point returns a
//! `Result`/`String` and never panics: decode errors become
//! [`PreviewError::Decode`], and a memory limit caps the decoder so a crafted
//! header cannot allocate unbounded memory.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;

/// The kind of preview to produce (mirrors the FFI `PreviewKind`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    /// A downscaled PNG thumbnail (raster formats, or an embedded JPEG).
    Thumbnail,
    /// A full-resolution PNG re-encode (raster formats, or an embedded JPEG).
    Image,
    /// UTF-8-lossy text.
    Text,
    /// A classic hex dump.
    Hex,
}

/// A preview could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    /// The bytes could not be decoded as the expected format.
    #[error("decode: {0}")]
    Decode(String),
    /// No bytes were supplied.
    #[error("no bytes to preview")]
    Empty,
}

/// Upper bound on the decoder's working-set allocation (defends against
/// decompression bombs from corrupt carved headers). 256 MiB is comfortably
/// above any real photo yet bounds a hostile one.
const MAX_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

/// The lowercase extensions this crate can decode as a raster image in-process.
/// HEIC/RAW are decoded from their embedded JPEG (which *is* one of these) or,
/// for a full-resolution HEIC, Swift-side via ImageIO.
#[must_use]
pub fn can_decode_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp"
    )
}

/// Decode a raster image and re-encode it as a PNG thumbnail whose longest side
/// is at most `max_px` (aspect preserved). `max_px == 0` is treated as "no
/// downscale" — equivalent to [`image_full_png`].
///
/// Accepts JPEG / PNG / GIF / BMP / WebP bytes, or the embedded JPEG of a
/// HEIC/RAW file.
pub fn image_thumbnail(bytes: &[u8], max_px: u32) -> Result<Vec<u8>, PreviewError> {
    if bytes.is_empty() {
        return Err(PreviewError::Empty);
    }
    let img = decode(bytes)?;
    let img = if max_px == 0 {
        img
    } else {
        // `thumbnail` preserves aspect ratio and never upscales past the source.
        img.thumbnail(max_px, max_px)
    };
    encode_png(&img)
}

/// Decode a raster image and re-encode it as a full-resolution PNG.
pub fn image_full_png(bytes: &[u8]) -> Result<Vec<u8>, PreviewError> {
    image_thumbnail(bytes, 0)
}

/// A UTF-8-lossy text preview of the first `max_bytes` bytes. Control bytes
/// other than tab/newline/carriage-return are shown as the Unicode replacement
/// character so a binary misfiled as text stays legible rather than emitting
/// raw control codes.
#[must_use]
pub fn text_preview(bytes: &[u8], max_bytes: usize) -> String {
    let slice = &bytes[..bytes.len().min(max_bytes)];
    String::from_utf8_lossy(slice)
        .chars()
        .map(|c| {
            if c == '\u{FFFD}' || c == '\t' || c == '\n' || c == '\r' || !c.is_control() {
                c
            } else {
                '\u{FFFD}'
            }
        })
        .collect()
}

/// A classic `hexdump -C`-style preview of the first `max_bytes` bytes:
/// `OFFSET  hex bytes  |ascii|`, 16 bytes per row.
#[must_use]
pub fn hex_preview(bytes: &[u8], max_bytes: usize) -> String {
    let slice = &bytes[..bytes.len().min(max_bytes)];
    let mut out = String::with_capacity(slice.len() / 16 * 80 + 16);
    for (row, chunk) in slice.chunks(16).enumerate() {
        let base = row * 16;
        out.push_str(&format!("{base:08x}  "));
        for i in 0..16 {
            if let Some(b) = chunk.get(i) {
                out.push_str(&format!("{b:02x} "));
            } else {
                out.push_str("   ");
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for b in chunk {
            let c = *b;
            out.push(if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                '.'
            });
        }
        out.push_str("|\n");
    }
    out
}

/// Decode raster bytes with a bounded decoder (format guessed from content,
/// allocation capped against decompression bombs).
fn decode(bytes: &[u8]) -> Result<image::DynamicImage, PreviewError> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| PreviewError::Decode(e.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_ALLOC_BYTES);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| PreviewError::Decode(e.to_string()))
}

/// Encode a decoded image to PNG bytes.
fn encode_png(img: &image::DynamicImage) -> Result<Vec<u8>, PreviewError> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| PreviewError::Decode(e.to_string()))?;
    Ok(out.into_inner())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A tiny 2x2 PNG produced by the encoder itself, used as decode input.
    fn tiny_png() -> Vec<u8> {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            2,
            image::Rgba([10, 20, 30, 255]),
        ));
        encode_png(&img).expect("encode")
    }

    #[test]
    fn thumbnail_roundtrips_and_downscales() {
        // A 64x32 source, thumbnailed to <= 16 px longest side.
        let src = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            32,
            image::Rgb([1, 2, 3]),
        ));
        let png = encode_png(&src).expect("encode");
        let out = image_thumbnail(&png, 16).expect("thumb");
        let decoded = decode(&out).expect("decode out");
        assert!(decoded.width() <= 16 && decoded.height() <= 16);
        assert!(decoded.width() >= 1 && decoded.height() >= 1);
    }

    #[test]
    fn full_png_preserves_dimensions() {
        let png = tiny_png();
        let out = image_full_png(&png).expect("full");
        let decoded = decode(&out).expect("decode");
        assert_eq!((decoded.width(), decoded.height()), (4, 2));
    }

    #[test]
    fn garbage_is_a_decode_error_not_a_panic() {
        let junk = vec![0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        assert!(matches!(
            image_thumbnail(&junk, 32),
            Err(PreviewError::Decode(_))
        ));
    }

    #[test]
    fn empty_input() {
        assert!(matches!(image_thumbnail(&[], 32), Err(PreviewError::Empty)));
    }

    #[test]
    fn text_preview_sanitizes_and_truncates() {
        let bytes = b"hello\tworld\n\x00\x01binary";
        let t = text_preview(bytes, 6);
        assert_eq!(t, "hello\t");
        let full = text_preview(bytes, 1000);
        assert!(full.starts_with("hello\tworld\n"));
        assert!(full.contains('\u{FFFD}')); // NUL/control replaced
    }

    #[test]
    fn hex_preview_shape() {
        let bytes: Vec<u8> = (0u8..20).collect();
        let h = hex_preview(&bytes, 1000);
        let first = h.lines().next().unwrap_or_default();
        assert!(first.starts_with("00000000  "));
        assert!(first.contains("00 01 02 03 04 05 06 07  08"));
        assert!(h.lines().count() == 2); // 20 bytes → 2 rows
    }

    #[test]
    fn ext_gate() {
        assert!(can_decode_ext("JPG"));
        assert!(can_decode_ext("webp"));
        assert!(!can_decode_ext("heic"));
        assert!(!can_decode_ext("cr2"));
    }
}
