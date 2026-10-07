//! XMP in JPEG, carried in an APP1 segment.
//!
//! APP1 is shared with EXIF, so segments are matched on the Adobe namespace
//! header rather than on the marker alone — matching on `FFE1` would clobber the
//! EXIF block.

use crate::error::{Error, Result};

/// The null-terminated header that identifies an XMP APP1 segment.
pub const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

const MARKER_PREFIX: u8 = 0xff;
const APP1: u8 = 0xe1;
const APP0: u8 = 0xe0;
const SOI: u8 = 0xd8;
const EOI: u8 = 0xd9;
const SOS: u8 = 0xda;

/// A JPEG segment, located by byte offsets into the file.
#[derive(Debug, Clone, Copy)]
struct Segment {
    marker: u8,
    /// Offset of the `FF` that introduces the segment.
    start: usize,
    /// Offset of the segment's payload.
    data_start: usize,
    /// Offset one past the segment.
    end: usize,
}

fn segments(bytes: &[u8]) -> Result<Vec<Segment>> {
    if bytes.len() < 4 || bytes[0] != MARKER_PREFIX || bytes[1] != SOI {
        return Err(Error::Corrupt("not a JPEG: missing SOI".into()));
    }

    let mut out = Vec::new();
    let mut i = 2usize;
    // Reaching the scan header is what tells us the segment table was complete.
    // Without this, a truncated file would parse as a short but valid segment
    // list, and the caller would be told the image simply has no XMP.
    let mut reached_scan = false;
    while i < bytes.len() {
        if bytes[i] != MARKER_PREFIX {
            return Err(Error::Corrupt(format!(
                "expected a marker at offset {i}, found {:#04x}",
                bytes[i]
            )));
        }
        if i + 1 >= bytes.len() {
            return Err(Error::Corrupt("file ends in the middle of a marker".into()));
        }
        let marker = bytes[i + 1];

        // Standalone markers carry no length field.
        if marker == SOI || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            i += 2;
            continue;
        }
        // Start of scan: entropy-coded data follows, which is not segment
        // structured.
        if marker == SOS || marker == EOI {
            reached_scan = true;
            break;
        }
        if i + 4 > bytes.len() {
            return Err(Error::Corrupt("truncated segment header".into()));
        }

        let length = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        if length < 2 {
            return Err(Error::Corrupt(format!(
                "segment at {i} declares length {length}"
            )));
        }
        let end = i + 2 + length;
        if end > bytes.len() {
            return Err(Error::Corrupt(format!(
                "segment at {i} runs past the end of the file"
            )));
        }
        out.push(Segment {
            marker,
            start: i,
            data_start: i + 2 + 2,
            end,
        });
        i = end;
    }

    if !reached_scan {
        return Err(Error::Corrupt(
            "file ends before its start-of-scan marker".into(),
        ));
    }
    Ok(out)
}

fn is_xmp(bytes: &[u8], segment: &Segment) -> bool {
    segment.marker == APP1
        && bytes
            .get(segment.data_start..segment.data_start + XMP_HEADER.len())
            .is_some_and(|header| header == XMP_HEADER)
}

pub fn extract(bytes: &[u8]) -> Result<Option<String>> {
    for segment in segments(bytes)? {
        if is_xmp(bytes, &segment) {
            let start = segment.data_start + XMP_HEADER.len();
            return Ok(Some(
                String::from_utf8_lossy(&bytes[start..segment.end]).into_owned(),
            ));
        }
    }
    Ok(None)
}

pub fn embed(bytes: &[u8], packet: &str) -> Result<Vec<u8>> {
    let existing = segments(bytes)?;
    let xmp = existing
        .iter()
        .copied()
        .find(|segment| is_xmp(bytes, segment));

    let payload_len = XMP_HEADER.len() + packet.len();
    // A segment's length field counts itself, so the payload cannot exceed
    // 65533 bytes.
    if payload_len + 2 > usize::from(u16::MAX) {
        return Err(Error::TooLarge(payload_len + 2));
    }

    let mut segment = Vec::with_capacity(payload_len + 4);
    segment.extend_from_slice(&[MARKER_PREFIX, APP1]);
    segment.extend_from_slice(&((payload_len + 2) as u16).to_be_bytes());
    segment.extend_from_slice(XMP_HEADER);
    segment.extend_from_slice(packet.as_bytes());

    // Keep the first packet's position, or insert after JFIF for a new packet.
    let insert_at = xmp.map_or_else(
        || {
            existing
                .iter()
                .filter(|s| s.marker == APP0)
                .map(|s| s.end)
                .next_back()
                .unwrap_or(2)
        },
        |old| old.start,
    );
    let mut out = Vec::with_capacity(bytes.len() + segment.len());
    out.extend_from_slice(&bytes[..insert_at]);
    out.extend_from_slice(&segment);
    let mut cursor = insert_at;
    for old in existing.iter().filter(|s| is_xmp(bytes, s)) {
        out.extend_from_slice(&bytes[cursor..old.start]);
        cursor = old.end;
    }
    out.extend_from_slice(&bytes[cursor..]);
    Ok(out)
}

/// Widens the check used by `segments`, so the CLI and tests can assert that a
/// rewrite left the rest of the file untouched.
pub fn segment_offsets(bytes: &[u8]) -> Result<Vec<(u8, usize, usize)>> {
    Ok(segments(bytes)?
        .into_iter()
        .map(|s| (s.marker, s.start, s.end))
        .collect())
}
