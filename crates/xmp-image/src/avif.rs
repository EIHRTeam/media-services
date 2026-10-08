//! XMP in AVIF (and HEIF), carried as an `mime` item with content type
//! `application/rdf+xml`.
//!
//! AVIF is ISO BMFF, so the metadata is not a tagged block in a stream — it is
//! an *item* described by `iinf` (what it is) and `iloc` (where its bytes are).
//! Adding one means new entries in both tables, and because `meta` precedes
//! `mdat`, growing it shifts every absolute file offset that follows.
//!
//! `iloc` in these files uses construction method 0, meaning offsets are
//! absolute from the start of the file, so a rewrite has to move them by the
//! exact amount `meta` grew. Getting that wrong produces a file that still
//! opens and shows a picture — with the image data read from the wrong place
//! only once something decodes it.

use crate::error::{Error, Result};

const XMP_CONTENT_TYPE: &str = "application/rdf+xml";
const MIME_ITEM: &[u8; 4] = b"mime";

/// A box, located by byte offsets.
#[derive(Debug, Clone, Copy)]
struct Box {
    kind: [u8; 4],
    /// Offset of the box's size field.
    start: usize,
    /// Offset of the box's payload.
    data_start: usize,
    /// Offset one past the box.
    end: usize,
}

impl Box {
    fn size(&self) -> usize {
        self.end - self.start
    }
}

fn name(kind: &[u8; 4]) -> String {
    String::from_utf8_lossy(kind).into_owned()
}

/// Walks the boxes in `[start, end)`.
fn boxes(bytes: &[u8], start: usize, end: usize) -> Result<Vec<Box>> {
    let mut out = Vec::new();
    let mut i = start;
    while i + 8 <= end {
        let size32 = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let kind = [bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]];
        // size 1 means a 64-bit size follows the type; size 0 means "to end of
        // file", which only a trailing `mdat` is allowed to do.
        let (size, header) = match size32 {
            0 => (end - i, 8),
            1 => {
                if i + 16 > end {
                    return Err(Error::Corrupt("truncated 64-bit box size".into()));
                }
                let high =
                    u32::from_be_bytes([bytes[i + 8], bytes[i + 9], bytes[i + 10], bytes[i + 11]]);
                let low = u32::from_be_bytes([
                    bytes[i + 12],
                    bytes[i + 13],
                    bytes[i + 14],
                    bytes[i + 15],
                ]);
                (
                    usize::try_from((high as u64) << 32 | low as u64)
                        .map_err(|_| Error::Corrupt("box size exceeds address space".into()))?,
                    16,
                )
            }
            other => (other as usize, 8),
        };
        // A declared size is attacker-controlled, so it is combined with
        // checked arithmetic: a box claiming to be 2^63 bytes long must not wrap
        // its way past this test.
        let box_end = i
            .checked_add(size)
            .filter(|_| size >= header)
            .ok_or_else(|| {
                Error::Corrupt(format!("{} box at {i} declares size {size}", name(&kind)))
            })?;
        if box_end > end {
            return Err(Error::Corrupt(format!(
                "{} box at {i} declares size {size}",
                name(&kind)
            )));
        }
        out.push(Box {
            kind,
            start: i,
            data_start: i + header,
            end: box_end,
        });
        i = box_end;
    }
    Ok(out)
}

fn find<'a>(list: &'a [Box], kind: &[u8; 4]) -> Option<&'a Box> {
    list.iter().find(|b| &b.kind == kind)
}

/// `meta` is a FullBox, so its children start after a 4-byte version/flags.
fn meta_children(bytes: &[u8], meta: &Box) -> Result<Vec<Box>> {
    if meta.data_start + 4 > meta.end {
        return Err(Error::Corrupt("meta box is too short".into()));
    }
    boxes(bytes, meta.data_start + 4, meta.end)
}

/// One entry of the `iinf` table.
struct ItemInfo {
    id: u16,
    item_type: [u8; 4],
    content_type: Option<String>,
}

fn parse_iinf(bytes: &[u8], iinf: &Box) -> Result<Vec<ItemInfo>> {
    let p = iinf.data_start;
    if p + 6 > iinf.end {
        return Err(Error::Corrupt("iinf is too short".into()));
    }
    let version = bytes[p];
    let count_width = if version == 0 { 2 } else { 4 };
    let count = read_uint(&bytes[..iinf.end], p + 4, count_width)
        .ok_or_else(|| Error::Corrupt("iinf count is truncated".into()))? as usize;
    let mut items = Vec::new();

    // Below version 2, `infe` is a plain box; at 2 and above it is a FullBox
    // with the item id and type inline.
    let mut i = p + 4 + count_width;
    while i + 8 <= iinf.end && items.len() < count {
        let size =
            u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let kind = [bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]];
        if size < 8 || i.checked_add(size).is_none_or(|end| end > iinf.end) {
            return Err(Error::Corrupt("iinf contains a malformed entry".into()));
        }
        if &kind == b"infe" {
            let body = &bytes[i + 8..i + size];
            if body.len() < 4 {
                return Err(Error::Corrupt("infe is too short".into()));
            }
            let infe_version = body[0];
            if infe_version > 2 {
                return Err(Error::Unsupported(
                    "infe versions above 2 are unsupported".into(),
                ));
            }
            if infe_version == 2 {
                if body.len() < 12 {
                    return Err(Error::Corrupt("infe version 2 entry is too short".into()));
                }
                let id = u16::from_be_bytes([body[4], body[5]]);
                let item_type = [body[8], body[9], body[10], body[11]];
                // item_name, then content_type when the item is a `mime` one.
                let rest = &body[12..];
                let (_, after_name) = split_c_string(rest);
                let content_type = after_name.and_then(|tail| {
                    let (value, _) = split_c_string(tail);
                    (!value.is_empty()).then(|| value.to_string())
                });
                items.push(ItemInfo {
                    id,
                    item_type,
                    content_type,
                });
            } else if body.len() >= 6 {
                items.push(ItemInfo {
                    id: u16::from_be_bytes([body[4], body[5]]),
                    item_type: *b"\0\0\0\0",
                    content_type: None,
                });
            }
        }
        i += size;
    }
    if items.len() != count {
        return Err(Error::Corrupt(
            "iinf item count does not match its entries".into(),
        ));
    }
    Ok(items)
}

fn split_c_string(bytes: &[u8]) -> (String, Option<&[u8]>) {
    match bytes.iter().position(|&b| b == 0) {
        Some(at) => (
            String::from_utf8_lossy(&bytes[..at]).into_owned(),
            Some(&bytes[at + 1..]),
        ),
        None => (String::from_utf8_lossy(bytes).into_owned(), None),
    }
}

/// One item's location, with the field widths the file chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Extent {
    index: u64,
    offset: u64,
    length: u64,
}

#[derive(Debug, Clone)]
struct ItemLocation {
    id: u16,
    construction_method: u8,
    base_offset: u64,
    extents: Vec<Extent>,
}

#[derive(Debug, Clone, Copy)]
struct IlocLayout {
    version: u8,
    offset_size: usize,
    length_size: usize,
    base_offset_size: usize,
    index_size: usize,
}

fn read_uint(bytes: &[u8], at: usize, size: usize) -> Option<u64> {
    if size > 8 {
        return None;
    }
    let mut value = 0u64;
    for byte in bytes.get(at..at.checked_add(size)?)? {
        value = (value << 8) | u64::from(*byte);
    }
    Some(value)
}

/// Writes `value` in `size` bytes, keeping the field width the file used.
///
/// `size` comes from the file, so it is clamped: a width above 8 would underflow
/// the slice, and one of 0 would silently drop the value.
fn write_uint(out: &mut Vec<u8>, value: u64, size: usize) -> Result<()> {
    if size > 8 || (size < 8 && value >= (1u64 << (size * 8))) {
        return Err(Error::Unsupported(
            "iloc value does not fit its field width".into(),
        ));
    }
    out.extend_from_slice(&value.to_be_bytes()[8 - size..]);
    Ok(())
}

fn parse_iloc(bytes: &[u8], iloc: &Box) -> Result<(IlocLayout, Vec<ItemLocation>)> {
    // Reads must stay within this box, even when another box follows it.
    let bytes = &bytes[..iloc.end];
    let p = iloc.data_start;
    if p + 6 > iloc.end {
        return Err(Error::Corrupt("iloc is too short".into()));
    }
    let version = bytes[p];
    let header_size = match version {
        0 | 1 => 8,
        2 => 10,
        _ => return Err(Error::Unsupported(format!("iloc version {version}"))),
    };
    if iloc.end - p < header_size {
        return Err(Error::Corrupt("iloc item count is truncated".into()));
    }
    let sizes = bytes[p + 4];
    let sizes2 = bytes[p + 5];
    let layout = IlocLayout {
        version,
        offset_size: usize::from(sizes >> 4),
        length_size: usize::from(sizes & 0x0f),
        base_offset_size: usize::from(sizes2 >> 4),
        index_size: usize::from(if version == 0 { 0 } else { sizes2 & 0x0f }),
    };
    // Version 2 widens the item count to 32 bits.
    let (count, mut i) = if version < 2 {
        (
            u16::from_be_bytes([bytes[p + 6], bytes[p + 7]]) as usize,
            p + 8,
        )
    } else {
        (
            u32::from_be_bytes([bytes[p + 6], bytes[p + 7], bytes[p + 8], bytes[p + 9]]) as usize,
            p + 10,
        )
    };

    // Allocate only as entries are successfully parsed, not from the file's count.
    let mut items = Vec::new();
    for _ in 0..count {
        let id = read_uint(bytes, i, if version < 2 { 2 } else { 4 })
            .ok_or_else(|| Error::Corrupt("iloc entry is truncated".into()))?;
        let id = u16::try_from(id)
            .map_err(|_| Error::Unsupported("32-bit item IDs are unsupported".into()))?;
        i += if version < 2 { 2 } else { 4 };

        let construction_method = if version == 0 {
            0
        } else {
            let method = read_uint(bytes, i, 2)
                .ok_or_else(|| Error::Corrupt("iloc construction method is truncated".into()))?;
            i += 2;
            (method & 0x0f) as u8
        };
        let reference = read_uint(bytes, i, 2)
            .ok_or_else(|| Error::Corrupt("iloc data reference is truncated".into()))?;
        if reference != 0 {
            return Err(Error::Unsupported(
                "external item data is unsupported".into(),
            ));
        }
        i += 2;

        let base_offset = read_uint(bytes, i, layout.base_offset_size)
            .ok_or_else(|| Error::Corrupt("iloc base_offset is truncated".into()))?;
        i += layout.base_offset_size;

        let extent_count = read_uint(bytes, i, 2)
            .ok_or_else(|| Error::Corrupt("iloc extent_count is truncated".into()))?
            as usize;
        i += 2;

        let mut extents = Vec::new();
        for _ in 0..extent_count {
            // Version 1 adds a per-extent index.
            let index = read_uint(bytes, i, layout.index_size)
                .ok_or_else(|| Error::Corrupt("iloc extent index is truncated".into()))?;
            i += layout.index_size;
            let offset = read_uint(bytes, i, layout.offset_size)
                .ok_or_else(|| Error::Corrupt("iloc extent offset is truncated".into()))?;
            i += layout.offset_size;
            let length = read_uint(bytes, i, layout.length_size)
                .ok_or_else(|| Error::Corrupt("iloc extent length is truncated".into()))?;
            i += layout.length_size;
            extents.push(Extent {
                index,
                offset,
                length,
            });
        }

        items.push(ItemLocation {
            id,
            construction_method,
            base_offset,
            extents,
        });
    }
    Ok((layout, items))
}

fn serialise_iloc(layout: IlocLayout, items: &[ItemLocation]) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.push(layout.version);
    payload.extend_from_slice(&[0, 0, 0]); // flags
    payload.push((layout.offset_size as u8) << 4 | layout.length_size as u8);
    payload.push((layout.base_offset_size as u8) << 4 | layout.index_size as u8);
    if layout.version < 2 {
        payload.extend_from_slice(
            &u16::try_from(items.len())
                .map_err(|_| Error::Unsupported("iloc item count overflows".into()))?
                .to_be_bytes(),
        );
    } else {
        payload.extend_from_slice(&(items.len() as u32).to_be_bytes());
    }

    for item in items {
        if layout.version < 2 {
            payload.extend_from_slice(&item.id.to_be_bytes());
        } else {
            payload.extend_from_slice(&u32::from(item.id).to_be_bytes());
        }
        // data_reference_index, with the construction method in the low nibble
        // when the version supports it.
        if layout.version == 0 {
            payload.extend_from_slice(&[0, 0]);
        } else {
            payload.extend_from_slice(&[0, item.construction_method, 0, 0]);
        }
        write_uint(&mut payload, item.base_offset, layout.base_offset_size)?;
        payload.extend_from_slice(
            &u16::try_from(item.extents.len())
                .map_err(|_| Error::Unsupported("extent count overflows".into()))?
                .to_be_bytes(),
        );
        for extent in &item.extents {
            // Index fields are padded; they are unused for single-file items.
            write_uint(&mut payload, extent.index, layout.index_size)?;
            write_uint(&mut payload, extent.offset, layout.offset_size)?;
            write_uint(&mut payload, extent.length, layout.length_size)?;
        }
    }
    boxed(b"iloc", &payload)
}

fn boxed(kind: &[u8; 4], payload: &[u8]) -> Result<Vec<u8>> {
    let length = payload
        .len()
        .checked_add(8)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Error::Unsupported("box length overflows".into()))?;
    let mut out = Vec::with_capacity(length as usize);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    Ok(out)
}

fn mime_infe(item_id: u16) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&[2, 0, 0, 0]); // version 2, flags
    payload.extend_from_slice(&item_id.to_be_bytes());
    payload.extend_from_slice(&0u16.to_be_bytes()); // item_protection_index
    payload.extend_from_slice(MIME_ITEM);
    payload.push(0); // item_name: empty
    payload.extend_from_slice(XMP_CONTENT_TYPE.as_bytes());
    payload.push(0);
    boxed(b"infe", &payload)
}

/// The XMP item's id, and where its bytes are.
fn xmp_item(
    bytes: &[u8],
    items: &[ItemInfo],
    locations: &[ItemLocation],
) -> Option<(u16, Vec<u8>)> {
    let info = items.iter().find(|i| {
        &i.item_type == MIME_ITEM && i.content_type.as_deref() == Some(XMP_CONTENT_TYPE)
    })?;
    let location = locations.iter().find(|l| l.id == info.id)?;
    let mut data = Vec::new();
    for extent in &location.extents {
        // Only file-absolute offsets are resolvable without knowing where an
        // `idat` box starts, and a wrong guess here would return image bytes as
        // metadata.
        if location.construction_method != 0 {
            return None;
        }
        let start = usize::try_from(location.base_offset.checked_add(extent.offset)?).ok()?;
        let end = start.checked_add(usize::try_from(extent.length).ok()?)?;
        data.extend_from_slice(bytes.get(start..end)?);
    }
    Some((info.id, data))
}

/// Everything a rewrite needs to know about the file's layout.
struct Layout {
    top: Vec<Box>,
    meta_index: usize,
    meta_children: Vec<Box>,
    locations: Vec<ItemLocation>,
    iloc_layout: IlocLayout,
    xmp_data: Option<Vec<u8>>,
    next_item_id: u16,
}

fn inspect(bytes: &[u8]) -> Result<Layout> {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return Err(Error::Corrupt("not an ISO BMFF file: no ftyp box".into()));
    }
    let top = boxes(bytes, 0, bytes.len())?;
    let meta_index = top
        .iter()
        .position(|b| &b.kind == b"meta")
        .ok_or_else(|| Error::Unsupported("this AVIF has no meta box".into()))?;
    let meta = top[meta_index];
    let children = meta_children(bytes, &meta)?;

    let iinf = find(&children, b"iinf")
        .ok_or_else(|| Error::Unsupported("this AVIF has no iinf box".into()))?;
    let iloc = find(&children, b"iloc")
        .ok_or_else(|| Error::Unsupported("this AVIF has no iloc box".into()))?;

    let items = parse_iinf(bytes, iinf)?;
    let (iloc_layout, locations) = parse_iloc(bytes, iloc)?;
    let xmp_data = xmp_item(bytes, &items, &locations).map(|(_, data)| data);
    let next_item_id = items
        .iter()
        .map(|i| i.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| Error::Unsupported("the item id space is exhausted".into()))?;

    Ok(Layout {
        top,
        meta_index,
        meta_children: children,
        locations,
        iloc_layout,
        xmp_data,
        next_item_id,
    })
}

pub fn extract(bytes: &[u8]) -> Result<Option<String>> {
    let layout = inspect(bytes)?;
    Ok(layout
        .xmp_data
        .map(|data| String::from_utf8_lossy(&data).into_owned()))
}

/// Rebuilds `iinf` with one more entry.
fn rebuild_iinf(bytes: &[u8], iinf: &Box, extra: &[u8]) -> Result<Vec<u8>> {
    let p = iinf.data_start;
    if p + 6 > iinf.end {
        return Err(Error::Corrupt("iinf is too short".into()));
    }
    let version = bytes[p];
    let mut payload = vec![version, 0, 0, 0];
    // Version 0 counts entries in 16 bits; later versions in 32.
    let entries_start = if version == 0 {
        let count = u16::from_be_bytes([bytes[p + 4], bytes[p + 5]]);
        payload.extend_from_slice(
            &count
                .checked_add(1)
                .ok_or_else(|| Error::Unsupported("iinf count overflows".into()))?
                .to_be_bytes(),
        );
        p + 6
    } else {
        let count = u32::from_be_bytes([bytes[p + 4], bytes[p + 5], bytes[p + 6], bytes[p + 7]]);
        payload.extend_from_slice(
            &count
                .checked_add(1)
                .ok_or_else(|| Error::Unsupported("iinf count overflows".into()))?
                .to_be_bytes(),
        );
        p + 8
    };
    payload.extend_from_slice(&bytes[entries_start..iinf.end]);
    payload.extend_from_slice(extra);
    boxed(b"iinf", &payload)
}

/// Rebuilds `meta`, substituting the two tables that changed.
fn rebuild_meta(
    bytes: &[u8],
    meta: &Box,
    children: &[Box],
    iinf: Option<&[u8]>,
    iloc: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(meta.size());
    payload.extend_from_slice(&bytes[meta.data_start..meta.data_start + 4]);
    for child in children {
        let replacement = match &child.kind {
            b"iinf" => iinf,
            b"iloc" => iloc,
            _ => None,
        };
        match replacement {
            Some(data) => payload.extend_from_slice(data),
            None => payload.extend_from_slice(&bytes[child.start..child.end]),
        }
    }
    boxed(b"meta", &payload)
}

pub fn embed(bytes: &[u8], packet: &str) -> Result<Vec<u8>> {
    let layout = inspect(bytes)?;
    let meta = layout.top[layout.meta_index];

    if layout.xmp_data.is_some() {
        // Replacing would mean removing the old item's bytes from wherever they
        // sit inside the file, and leaving them would keep serving stale
        // metadata to anything that reads the first match.
        return Err(Error::Unsupported(
            "an XMP item is already present; replacing it in an AVIF is not supported yet".into(),
        ));
    }

    // Only file-absolute offsets can be relocated with confidence. A
    // compression-relative item would also move, but its offset is expressed
    // against a box we are resizing, so the shift is not ours to reason about.
    for location in &layout.locations {
        if location.construction_method != 0 {
            return Err(Error::Unsupported(
                "this AVIF stores item data by construction method other than file offset".into(),
            ));
        }
    }

    let iinf = find(&layout.meta_children, b"iinf")
        .ok_or_else(|| Error::Unsupported("this AVIF has no iinf box".into()))?;

    let new_infe = mime_infe(layout.next_item_id)?;
    let new_iinf = rebuild_iinf(bytes, iinf, &new_infe)?;
    let mut locations = layout.locations;
    // Widths are fixed. A zero placeholder establishes metadata growth without
    // building or copying any image payload.
    locations.push(ItemLocation {
        id: layout.next_item_id,
        construction_method: 0,
        base_offset: 0,
        extents: vec![Extent {
            index: 0,
            offset: 0,
            length: packet.len() as u64,
        }],
    });
    let probe_iloc = serialise_iloc(layout.iloc_layout, &locations)?;
    let old_iloc = find(&layout.meta_children, b"iloc").expect("inspect checked iloc");
    let delta = new_iinf
        .len()
        .checked_sub(iinf.size())
        .and_then(|n| {
            probe_iloc
                .len()
                .checked_sub(old_iloc.size())
                .and_then(|m| n.checked_add(m))
        })
        .ok_or_else(|| Error::Unsupported("metadata layout would shrink or overflow".into()))?;
    let moved_end = bytes
        .len()
        .checked_add(delta)
        .ok_or_else(|| Error::Corrupt("file length overflows".into()))?;
    let last = layout.top.last().copied();
    let append_box = last.is_none_or(|b| &b.kind != b"mdat");
    let xmp_at = moved_end
        .checked_add(if append_box { 8 } else { 0 })
        .ok_or_else(|| Error::Corrupt("XMP offset overflows".into()))?;
    let final_size = xmp_at
        .checked_add(packet.len())
        .ok_or_else(|| Error::Corrupt("file length overflows".into()))?;
    let old_count = locations.len() - 1;
    for location in &mut locations[..old_count] {
        let mut shift_states = Vec::new();
        for extent in &location.extents {
            let start = location
                .base_offset
                .checked_add(extent.offset)
                .ok_or_else(|| Error::Corrupt("extent offset overflows".into()))?;
            let end = start
                .checked_add(extent.length)
                .ok_or_else(|| Error::Corrupt("extent end overflows".into()))?;
            if end > bytes.len() as u64 || (start < meta.end as u64 && end > meta.start as u64) {
                return Err(Error::Unsupported(
                    "extent is out of file bounds or overlaps metadata".into(),
                ));
            }
            shift_states.push(start >= meta.end as u64);
        }
        let all_shift = !shift_states.is_empty() && shift_states.iter().all(|s| *s);
        if all_shift && layout.iloc_layout.base_offset_size != 0 && location.base_offset != 0 {
            location.base_offset = location
                .base_offset
                .checked_add(delta as u64)
                .ok_or_else(|| Error::Corrupt("base offset overflows".into()))?;
        } else {
            for (extent, shift) in location.extents.iter_mut().zip(shift_states) {
                if shift {
                    extent.offset = extent
                        .offset
                        .checked_add(delta as u64)
                        .ok_or_else(|| Error::Corrupt("extent offset overflows".into()))?;
                }
            }
        }
    }
    let new_item = locations.last_mut().expect("new XMP item");
    if layout.iloc_layout.base_offset_size != 0 {
        new_item.base_offset = xmp_at as u64;
    } else {
        new_item.extents[0].offset = xmp_at as u64;
    }
    let iloc = serialise_iloc(layout.iloc_layout, &locations)?;
    let new_meta = rebuild_meta(
        bytes,
        &meta,
        &layout.meta_children,
        Some(&new_iinf),
        Some(&iloc),
    )?;
    debug_assert_eq!(new_meta.len(), meta.size() + delta);
    let mut out = Vec::with_capacity(final_size);
    out.extend_from_slice(&bytes[..meta.start]);
    out.extend_from_slice(&new_meta);
    out.extend_from_slice(&bytes[meta.end..]);
    if append_box {
        let size = packet
            .len()
            .checked_add(8)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| Error::Unsupported("mdat size overflows".into()))?;
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(b"mdat");
    } else if let Some(last) = last {
        let at = last.start + if last.start >= meta.end { delta } else { 0 };
        let declared = u32::from_be_bytes(
            bytes[last.start..last.start + 4]
                .try_into()
                .expect("box header"),
        );
        let grown = last
            .size()
            .checked_add(packet.len())
            .ok_or_else(|| Error::Unsupported("mdat size overflows".into()))?;
        match declared {
            0 => {}
            1 => out[at + 8..at + 16].copy_from_slice(&(grown as u64).to_be_bytes()),
            _ => out[at..at + 4].copy_from_slice(
                &u32::try_from(grown)
                    .map_err(|_| Error::Unsupported("mdat size overflows".into()))?
                    .to_be_bytes(),
            ),
        }
    }
    out.extend_from_slice(packet.as_bytes());
    debug_assert_eq!(out.len(), final_size);
    Ok(out)
}
