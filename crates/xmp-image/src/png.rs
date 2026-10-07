//! XMP in PNG, carried in an uncompressed `iTXt` chunk.

use crate::crc32::crc32;
use crate::error::{Error, Result};

/// The keyword Adobe tools use for XMP in PNG.
pub const XMP_KEYWORD: &str = "XML:com.adobe.xmp";

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkType(pub [u8; 4]);

impl ChunkType {
    pub const IHDR: Self = Self(*b"IHDR");
    pub const IDAT: Self = Self(*b"IDAT");
    pub const IEND: Self = Self(*b"IEND");
    pub const ITXT: Self = Self(*b"iTXt");
}

impl std::fmt::Display for ChunkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

struct Chunk {
    kind: ChunkType,
    start: usize,
    /// Offset of the chunk's data (after the length and type fields).
    data_start: usize,
    end: usize,
}

fn chunks(bytes: &[u8]) -> Result<Vec<Chunk>> {
    if !bytes.starts_with(SIGNATURE) {
        return Err(Error::Corrupt("not a PNG: bad signature".into()));
    }
    let mut out = Vec::new();
    let mut i = SIGNATURE.len();
    while i + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let kind = ChunkType([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]);
        let data_start = i + 8;
        let end = data_start
            .checked_add(length)
            .and_then(|end| end.checked_add(4)) // CRC
            .ok_or_else(|| Error::Corrupt(format!("{kind} chunk length overflows")))?;
        if end > bytes.len() {
            return Err(Error::Corrupt(format!(
                "{kind} chunk at {i} runs past the end of the file"
            )));
        }
        out.push(Chunk {
            kind,
            start: i,
            data_start,
            end,
        });
        i = end;
        if kind == ChunkType::IEND {
            break;
        }
    }
    Ok(out)
}

/// Splits an `iTXt` payload into its keyword and the fields after it.
fn parse_itxt(data: &[u8]) -> Option<(&[u8], u8, &[u8])> {
    let keyword_end = data.iter().position(|&b| b == 0)?;
    let keyword = &data[..keyword_end];
    let rest = data.get(keyword_end + 1..)?;
    if rest.len() < 2 {
        return None;
    }
    let (compression_flag, compression_method) = (rest[0], rest[1]);
    // Fields after the compression bytes are the language tag, the translated
    // keyword, then the text — each terminated except the last.
    let after_compression = &rest[2..];
    let language_end = after_compression.iter().position(|&b| b == 0)?;
    let after_language = &after_compression[language_end + 1..];
    let translated_end = after_language.iter().position(|&b| b == 0)?;
    let text = &after_language[translated_end + 1..];

    let _ = compression_method;
    Some((keyword, compression_flag, text))
}

fn is_xmp(data: &[u8]) -> bool {
    parse_itxt(data).is_some_and(|(keyword, _, _)| keyword == XMP_KEYWORD.as_bytes())
}

pub fn extract(bytes: &[u8]) -> Result<Option<String>> {
    for chunk in chunks(bytes)? {
        if chunk.kind != ChunkType::ITXT {
            continue;
        }
        let data = &bytes[chunk.data_start..chunk.end - 4];
        if !is_xmp(data) {
            continue;
        }
        let Some((_, compression_flag, text)) = parse_itxt(data) else {
            continue;
        };
        // Reporting a compressed packet as absent would look like "this image
        // has no XMP", which is a different and more misleading answer.
        if compression_flag != 0 {
            return Err(Error::Unsupported(
                "XMP is stored in a compressed iTXt chunk".into(),
            ));
        }
        return Ok(Some(String::from_utf8_lossy(text).into_owned()));
    }
    Ok(None)
}

fn itxt_chunk(packet: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(packet.len() + XMP_KEYWORD.len() + 5);
    data.extend_from_slice(XMP_KEYWORD.as_bytes());
    data.push(0); // keyword terminator
    data.push(0); // compression flag: uncompressed
    data.push(0); // compression method
    data.push(0); // language tag: empty
    data.push(0); // translated keyword: empty
    data.extend_from_slice(packet.as_bytes());

    let mut chunk = Vec::with_capacity(data.len() + 12);
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(&ChunkType::ITXT.0);
    chunk.extend_from_slice(&data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(&ChunkType::ITXT.0);
    crc_input.extend_from_slice(&data);
    chunk.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    chunk
}

pub fn embed(bytes: &[u8], packet: &str) -> Result<Vec<u8>> {
    let existing = chunks(bytes)?;
    let ihdr = existing
        .iter()
        .find(|c| c.kind == ChunkType::IHDR)
        .ok_or_else(|| Error::Corrupt("PNG has no IHDR chunk".into()))?;

    // Replace every existing XMP chunk with one authoritative packet.
    let stale = existing
        .iter()
        .find(|c| c.kind == ChunkType::ITXT && is_xmp(&bytes[c.data_start..c.end - 4]));

    let chunk = itxt_chunk(packet);
    let mut out = Vec::with_capacity(bytes.len() + chunk.len());
    // For a new packet, insert immediately after IHDR, before IDAT.
    let insert_at = stale.map_or(ihdr.end, |old| old.start);
    out.extend_from_slice(&bytes[..insert_at]);
    out.extend_from_slice(&chunk);
    let mut cursor = insert_at;
    for old in existing
        .iter()
        .filter(|c| c.kind == ChunkType::ITXT && is_xmp(&bytes[c.data_start..c.end - 4]))
    {
        out.extend_from_slice(&bytes[cursor..old.start]);
        cursor = old.end;
    }
    out.extend_from_slice(&bytes[cursor..]);
    Ok(out)
}

/// Chunk types in order, for tests asserting that a rewrite preserved the image.
pub fn chunk_types(bytes: &[u8]) -> Result<Vec<String>> {
    Ok(chunks(bytes)?
        .into_iter()
        .map(|c| c.kind.to_string())
        .collect())
}

/// Verifies every chunk's CRC. A rewrite that recomputes a CRC incorrectly
/// produces a file that decoders reject outright.
pub fn verify_crcs(bytes: &[u8]) -> Result<()> {
    for chunk in chunks(bytes)? {
        let crc_field = chunk.end - 4;
        let stored = u32::from_be_bytes([
            bytes[crc_field],
            bytes[crc_field + 1],
            bytes[crc_field + 2],
            bytes[crc_field + 3],
        ]);
        let computed = crc32(&bytes[chunk.start + 4..crc_field]);
        if stored != computed {
            return Err(Error::Corrupt(format!(
                "{} chunk has CRC {stored:#010x}, computed {computed:#010x}",
                chunk.kind
            )));
        }
    }
    Ok(())
}
