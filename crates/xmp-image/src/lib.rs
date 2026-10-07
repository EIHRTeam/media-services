//! Reading and writing XMP inside image containers.
//!
//! Each format stores XMP somewhere different — a JPEG APP1 segment, a PNG
//! `iTXt` chunk, a WebP RIFF chunk — and each has its own way of going wrong if
//! the rewrite is not careful. This crate owns those differences so callers deal
//! in one `extract` / `embed` pair.
//!
//! ```
//! # fn main() -> Result<(), xmp_image::Error> {
//! let png = xmp_image::tests_support::minimal_png();
//! let packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>";
//! let stamped = xmp_image::embed_xmp(&png, packet)?;
//! assert_eq!(xmp_image::extract_xmp(&stamped)?.as_deref(), Some(packet));
//! # Ok(())
//! # }
//! ```

pub mod avif;
pub mod crc32;
pub mod error;
pub mod jpeg;
pub mod png;
pub mod webp;

pub use error::{Error, Result};

/// A container we know how to put XMP into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
    Avif,
}

impl Format {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
            Self::Avif => "image/avif",
        }
    }

    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime {
            "image/jpeg" | "image/jpg" => Some(Self::Jpeg),
            "image/png" => Some(Self::Png),
            "image/webp" => Some(Self::WebP),
            "image/avif" => Some(Self::Avif),
            _ => None,
        }
    }
}

/// Identifies the format from its magic bytes.
pub fn detect(bytes: &[u8]) -> Result<Format> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok(Format::Jpeg);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(Format::Png);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Ok(Format::WebP);
    }
    // ISO BMFF: the brand may be in the major brand or only in the compatible
    // brands that follow it.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"avif" || brand == b"avis" {
            return Ok(Format::Avif);
        }
        // The compatible-brands list starts after the major brand, the minor
        // version and the first brand — so a file shorter than that has none.
        // `as_chunks` rather than `chunks_exact`: the remainder is discarded
        // either way, and this states the element size in the type.
        let (brands, _trailing) = bytes.get(16..).unwrap_or_default().as_chunks::<4>();
        if brands
            .iter()
            .any(|candidate| candidate == b"avif" || candidate == b"avis")
        {
            return Ok(Format::Avif);
        }
    }
    Err(Error::UnsupportedFormat)
}

/// Reads the XMP packet, if the image carries one.
pub fn extract_xmp(bytes: &[u8]) -> Result<Option<String>> {
    extract_xmp_as(bytes, detect(bytes)?)
}

pub fn extract_xmp_as(bytes: &[u8], format: Format) -> Result<Option<String>> {
    match format {
        Format::Jpeg => jpeg::extract(bytes),
        Format::Png => png::extract(bytes),
        Format::WebP => webp::extract(bytes),
        Format::Avif => avif::extract(bytes),
    }
}

/// Replaces or inserts the XMP packet, leaving the image data untouched.
pub fn embed_xmp(bytes: &[u8], packet: &str) -> Result<Vec<u8>> {
    embed_xmp_as(bytes, detect(bytes)?, packet)
}

pub fn embed_xmp_as(bytes: &[u8], format: Format, packet: &str) -> Result<Vec<u8>> {
    match format {
        Format::Jpeg => jpeg::embed(bytes, packet),
        Format::Png => png::embed(bytes, packet),
        Format::WebP => webp::embed(bytes, packet),
        Format::Avif => avif::embed(bytes, packet),
    }
}

/// Small byte builders used by the tests and the doc example.
#[doc(hidden)]
pub mod tests_support {
    use crate::crc32::crc32;

    /// A 1x1 PNG, built by hand so the tests need no binary fixtures.
    pub fn minimal_png() -> Vec<u8> {
        let mut out = Vec::from(&b"\x89PNG\r\n\x1a\n"[..]);
        // IHDR: 1x1, 8-bit greyscale, no interlacing.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        push_chunk(&mut out, b"IHDR", &ihdr);
        // One scanline: filter byte 0 then a single grey pixel.
        push_chunk(
            &mut out,
            b"IDAT",
            &[0x78, 0x9c, 0x62, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01],
        );
        push_chunk(&mut out, b"IEND", &[]);
        out
    }

    fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_input = Vec::from(&kind[..]);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
}
