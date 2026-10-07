//! XMP in WebP, carried in a `XMP ` RIFF chunk.
//!
//! The awkward part is not the chunk — it is that the XMP capability is
//! advertised by a flag bit in the `VP8X` header, a chunk which simple-format
//! files do not have. Adding XMP to such a file means synthesising a `VP8X`,
//! which in turn means recovering the canvas size from the image data.

use crate::error::{Error, Result};

const XMP_CHUNK: &[u8; 4] = b"XMP ";
const VP8X_CHUNK: &[u8; 4] = b"VP8X";
const VP8_CHUNK: &[u8; 4] = b"VP8 ";
const VP8L_CHUNK: &[u8; 4] = b"VP8L";

/// The `VP8X` flags byte advertises what the file carries. Bit 2 is XMP; c2pa
/// uses the same value.
const FLAG_XMP: u8 = 0x04;

#[derive(Debug, Clone)]
struct Chunk {
    fourcc: [u8; 4],
    /// Offset of the chunk's data.
    data_start: usize,
    /// Offset one past the chunk, including any pad byte.
    end: usize,
}

impl Chunk {
    fn data<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        &bytes[self.data_start..self.data_start + self.data_len(bytes)]
    }

    fn data_len(&self, bytes: &[u8]) -> usize {
        // The pad byte is not part of the declared length.
        let padded = self.end - self.data_start;
        if padded == 0 {
            return 0;
        }
        let declared = u32::from_le_bytes([
            bytes[self.data_start - 4],
            bytes[self.data_start - 3],
            bytes[self.data_start - 2],
            bytes[self.data_start - 1],
        ]) as usize;
        declared.min(padded)
    }
}

fn parse(bytes: &[u8]) -> Result<Vec<Chunk>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(Error::Corrupt("not a WebP: bad RIFF/WEBP header".into()));
    }
    let mut out = Vec::new();
    let mut i = 12usize;
    while i + 8 <= bytes.len() {
        let fourcc = [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]];
        let declared =
            u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        let data_start = i + 8;
        let data_end = data_start
            .checked_add(declared)
            .ok_or_else(|| Error::Corrupt("WebP chunk length overflows".into()))?;
        if data_end > bytes.len() {
            return Err(Error::Corrupt(format!(
                "{} chunk at {i} runs past the end of the file",
                String::from_utf8_lossy(&fourcc)
            )));
        }
        // Odd-sized chunks are followed by a pad byte.
        let end = data_end
            .checked_add(declared & 1)
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| Error::Corrupt("WebP chunk padding is truncated".into()))?;
        out.push(Chunk {
            fourcc,
            data_start,
            end,
        });
        i = end;
    }
    Ok(out)
}

/// Canvas dimensions from whichever header carries them.
fn canvas_size(bytes: &[u8], chunks: &[Chunk]) -> Result<(u32, u32)> {
    for chunk in chunks {
        let data = chunk.data(bytes);
        match &chunk.fourcc {
            VP8X_CHUNK => {
                if data.len() < 10 {
                    return Err(Error::Corrupt("VP8X chunk is too short".into()));
                }
                let width = u32::from_le_bytes([data[4], data[5], data[6], 0]) + 1;
                let height = u32::from_le_bytes([data[7], data[8], data[9], 0]) + 1;
                return Ok((width, height));
            }
            VP8_CHUNK => {
                // Frame tag (3 bytes), start code (3 bytes), then 14-bit
                // dimensions.
                if data.len() < 10 || data[3..6] != [0x9d, 0x01, 0x2a] {
                    return Err(Error::Corrupt("VP8 key frame header is malformed".into()));
                }
                let width = u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3fff);
                let height = u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3fff);
                return Ok((width, height));
            }
            VP8L_CHUNK => {
                if data.len() < 5 || data[0] != 0x2f {
                    return Err(Error::Corrupt("VP8L header is malformed".into()));
                }
                let bits = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
                let width = (bits & 0x3fff) + 1;
                let height = ((bits >> 14) & 0x3fff) + 1;
                return Ok((width, height));
            }
            _ => {}
        }
    }
    // Refusing beats guessing: a wrong canvas size produces a file that decoders
    // reject.
    Err(Error::Unsupported(
        "cannot determine the canvas size: no VP8X, VP8 or VP8L chunk found".into(),
    ))
}

pub fn extract(bytes: &[u8]) -> Result<Option<String>> {
    for chunk in parse(bytes)? {
        if &chunk.fourcc == XMP_CHUNK {
            return Ok(Some(
                String::from_utf8_lossy(chunk.data(bytes)).into_owned(),
            ));
        }
    }
    Ok(None)
}

fn write_chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

pub fn embed(bytes: &[u8], packet: &str) -> Result<Vec<u8>> {
    let chunks = parse(bytes)?;
    let (width, height) = canvas_size(bytes, &chunks)?;
    if width == 0 || height == 0 || width > 1 << 24 || height > 1 << 24 {
        return Err(Error::Corrupt(format!(
            "canvas size {width}x{height} is out of range"
        )));
    }

    let existing_vp8x = chunks.iter().find(|c| &c.fourcc == VP8X_CHUNK);

    let mut vp8x = Vec::with_capacity(10);
    let mut flags = existing_vp8x
        .map(|c| c.data(bytes).first().copied().unwrap_or(0))
        .unwrap_or(0);
    flags |= FLAG_XMP;
    vp8x.push(flags);
    vp8x.extend_from_slice(&[0, 0, 0]); // reserved
    vp8x.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    vp8x.extend_from_slice(&(height - 1).to_le_bytes()[..3]);

    let mut body = Vec::with_capacity(bytes.len() + packet.len() + 32);
    for chunk in &chunks {
        // The XMP chunk is rewritten, and an existing VP8X is replaced by the
        // one carrying the updated flags.
        if &chunk.fourcc == XMP_CHUNK || &chunk.fourcc == VP8X_CHUNK {
            continue;
        }
        body.extend_from_slice(&bytes[chunk.data_start - 8..chunk.end]);
    }

    // XMP belongs before the image data, so it goes at the front of the body.
    let mut with_headers = Vec::with_capacity(body.len() + packet.len() + 32);
    write_chunk(&mut with_headers, VP8X_CHUNK, &vp8x);
    write_chunk(&mut with_headers, XMP_CHUNK, packet.as_bytes());
    with_headers.extend_from_slice(&body);

    let mut out = Vec::with_capacity(with_headers.len() + 12);
    out.extend_from_slice(b"RIFF");
    // The RIFF size counts everything after the size field itself.
    out.extend_from_slice(&((with_headers.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&with_headers);
    Ok(out)
}

/// FourCCs in order, and the `VP8X` flags byte if there is one.
pub fn inspect(bytes: &[u8]) -> Result<(Vec<String>, Option<u8>)> {
    let chunks = parse(bytes)?;
    let flags = chunks
        .iter()
        .find(|c| &c.fourcc == VP8X_CHUNK)
        .and_then(|c| c.data(bytes).first().copied());
    Ok((
        chunks
            .iter()
            .map(|c| String::from_utf8_lossy(&c.fourcc).into_owned())
            .collect(),
        flags,
    ))
}
