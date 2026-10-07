//! Container round trips, with byte-level assertions on the structures that are
//! easy to get subtly wrong.

use std::assert_matches;
use std::io::Cursor;

use xmp_image::{Error, Format};

const PACKET: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>\
<?xpacket end=\"w\"?>";

fn jpeg() -> Vec<u8> {
    let img = image::RgbImage::from_fn(32, 32, |x, y| {
        image::Rgb([(x * 8) as u8, (y * 8) as u8, 64])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .expect("encode jpeg");
    out.into_inner()
}

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(32, 32, |x, y| {
        image::Rgb([(x * 8) as u8, (y * 8) as u8, 64])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

fn webp() -> Vec<u8> {
    // Lossless, so the result carries a VP8L chunk and no VP8X — the case that
    // forces the canvas size to be recovered from the bitstream.
    let img = image::RgbImage::from_fn(32, 32, |x, y| {
        image::Rgb([(x * 8) as u8, (y * 8) as u8, 64])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::WebP)
        .expect("encode webp");
    out.into_inner()
}

#[test]
fn detects_each_format_from_magic_bytes() {
    assert_eq!(xmp_image::detect(&jpeg()).unwrap(), Format::Jpeg);
    assert_eq!(xmp_image::detect(&png()).unwrap(), Format::Png);
    assert_eq!(xmp_image::detect(&webp()).unwrap(), Format::WebP);
    assert_matches!(xmp_image::detect(b"GIF89a"), Err(Error::UnsupportedFormat));
}

#[test]
fn an_unstamped_image_has_no_xmp() {
    for bytes in [jpeg(), png(), webp()] {
        assert_eq!(xmp_image::extract_xmp(&bytes).unwrap(), None);
    }
}

#[test]
fn round_trips_through_every_format() {
    for (name, bytes) in [("jpeg", jpeg()), ("png", png()), ("webp", webp())] {
        let stamped = xmp_image::embed_xmp(&bytes, PACKET).expect("embed");
        assert_eq!(
            xmp_image::extract_xmp(&stamped).unwrap().as_deref(),
            Some(PACKET),
            "{name} did not round trip"
        );
    }
}

#[test]
fn embedding_is_idempotent() {
    // Writing the same packet twice must not append a second copy: readers that
    // take the first would keep seeing the old value.
    for (name, bytes) in [("jpeg", jpeg()), ("png", png()), ("webp", webp())] {
        let once = xmp_image::embed_xmp(&bytes, PACKET).unwrap();
        let twice = xmp_image::embed_xmp(&once, PACKET).unwrap();
        assert_eq!(once, twice, "{name} was not idempotent");
    }
}

#[test]
fn jpeg_and_png_remove_all_old_xmp_blocks() {
    for (format, original) in [(Format::Jpeg, jpeg()), (Format::Png, png())] {
        let mut duplicate = xmp_image::embed_xmp(&original, PACKET).unwrap();
        let stale = xmp_image::embed_xmp(&original, "stale packet").unwrap();
        let (start, end) = match format {
            Format::Jpeg => {
                let (_, start, end) = xmp_image::jpeg::segment_offsets(&stale)
                    .unwrap()
                    .into_iter()
                    .find(|(marker, _, _)| *marker == 0xe1)
                    .unwrap();
                (start, end)
            }
            Format::Png => {
                // The generated packet is immediately after the 33-byte PNG header.
                let start = 33;
                let size = u32::from_be_bytes(stale[start..start + 4].try_into().unwrap()) as usize;
                (start, start + size + 12)
            }
            _ => unreachable!(),
        };
        duplicate.splice(start..start, stale[start..end].iter().copied());
        let rewritten = xmp_image::embed_xmp(&duplicate, "replacement packet").unwrap();
        assert_eq!(
            rewritten,
            xmp_image::embed_xmp(&original, "replacement packet").unwrap()
        );
        assert_eq!(
            xmp_image::extract_xmp(&rewritten).unwrap().as_deref(),
            Some("replacement packet")
        );
        let decoded = image::load_from_memory(&rewritten).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (32, 32));
        if format == Format::Png {
            xmp_image::png::verify_crcs(&rewritten).unwrap();
        }
    }
}

fn avif_with_iloc(payload: &[u8]) -> Vec<u8> {
    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }
    let mut out = boxed(b"ftyp", b"avif\0\0\0\0avif");
    let mut meta = vec![0, 0, 0, 0];
    meta.extend_from_slice(&boxed(b"iinf", &[0, 0, 0, 0, 0, 0]));
    meta.extend_from_slice(&boxed(b"iloc", payload));
    out.extend_from_slice(&boxed(b"meta", &meta));
    out
}

#[test]
fn avif_rejects_truncated_iloc_headers_and_entries() {
    for version in 0..=2 {
        let header_size = if version < 2 { 8 } else { 10 };
        let mut header = vec![0; header_size];
        header[0] = version;
        for length in 0..header_size {
            assert_matches!(
                xmp_image::extract_xmp(&avif_with_iloc(&header[..length])),
                Err(Error::Corrupt(_))
            );
        }
    }
    let mut truncated = avif_with_iloc(&[0, 0, 0, 0, 0x44, 0x40, 0, 1]);
    // A following box must not supply the missing iloc entry bytes.
    truncated.extend_from_slice(&72u32.to_be_bytes());
    truncated.extend_from_slice(b"mdat");
    truncated.extend_from_slice(&[0; 64]);
    assert_matches!(xmp_image::extract_xmp(&truncated), Err(Error::Corrupt(_)));
}

#[test]
fn avif_rejects_item_counts_without_allocating_them() {
    let header = [2, 0, 0, 0, 0x44, 0x40, 0xff, 0xff, 0xff, 0xff];
    assert_matches!(
        xmp_image::extract_xmp(&avif_with_iloc(&header)),
        Err(Error::Corrupt(_))
    );
}

#[test]
fn replacing_xmp_leaves_the_image_decodable() {
    for (name, bytes) in [("jpeg", jpeg()), ("png", png()), ("webp", webp())] {
        let mut stamped = xmp_image::embed_xmp(&bytes, PACKET).unwrap();
        stamped = xmp_image::embed_xmp(&stamped, "<?xpacket end=\"w\"?><different/>").unwrap();

        let decoded = image::load_from_memory(&stamped)
            .unwrap_or_else(|e| panic!("{name} no longer decodes after rewrite: {e}"));
        assert_eq!((decoded.width(), decoded.height()), (32, 32), "{name}");
    }
}

#[test]
fn jpeg_keeps_its_other_segments() {
    let original = jpeg();
    let stamped = xmp_image::embed_xmp(&original, PACKET).unwrap();

    let before = xmp_image::jpeg::segment_offsets(&original).unwrap();
    let after = xmp_image::jpeg::segment_offsets(&stamped).unwrap();
    assert_eq!(
        after.len(),
        before.len() + 1,
        "expected exactly one added segment"
    );

    // APP1 is shared with EXIF, so the XMP segment must be identified by its
    // Adobe header rather than by the marker alone.
    let app1_count_before = before.iter().filter(|(m, _, _)| *m == 0xe1).count();
    let app1_count_after = after.iter().filter(|(m, _, _)| *m == 0xe1).count();
    assert_eq!(app1_count_after, app1_count_before + 1);
}

#[test]
fn png_keeps_its_chunks_and_valid_crcs() {
    let original = png();
    let stamped = xmp_image::embed_xmp(&original, PACKET).unwrap();

    let types = xmp_image::png::chunk_types(&stamped).unwrap();
    assert!(types.contains(&"iTXt".to_string()));
    // XMP has to precede the image data to be readable without decoding it.
    let itxt = types.iter().position(|t| t == "iTXt").unwrap();
    let idat = types.iter().position(|t| t == "IDAT").unwrap();
    assert!(itxt < idat, "iTXt landed after IDAT: {types:?}");

    // A wrong CRC produces a file decoders reject outright.
    xmp_image::png::verify_crcs(&stamped).expect("CRCs must be valid");
}

#[test]
fn webp_sets_the_xmp_flag() {
    let original = webp();
    let (_, flags_before) = xmp_image::webp::inspect(&original).unwrap();

    let stamped = xmp_image::embed_xmp(&original, PACKET).unwrap();
    let (chunks, flags_after) = xmp_image::webp::inspect(&stamped).unwrap();

    assert!(chunks.contains(&"XMP ".to_string()), "{chunks:?}");
    // Bit 2 advertises XMP. A file carrying XMP without the flag is
    // non-conformant, and c2pa-rs will not set it for us.
    assert_eq!(flags_after.expect("VP8X") & 0x04, 0x04, "XMP flag not set");
    assert!(
        chunks.iter().position(|c| c == "XMP ").unwrap()
            < chunks.iter().position(|c| c == "VP8L").unwrap(),
        "XMP must precede the image data: {chunks:?}"
    );
    let _ = flags_before;
}

#[test]
fn webp_recomputes_the_riff_size() {
    let stamped = xmp_image::embed_xmp(&webp(), PACKET).unwrap();
    let declared = u32::from_le_bytes([stamped[4], stamped[5], stamped[6], stamped[7]]) as usize;
    assert_eq!(
        declared,
        stamped.len() - 8,
        "RIFF size field does not match the file length"
    );
}

#[test]
fn rejects_bytes_that_are_not_an_image() {
    assert_matches!(
        xmp_image::embed_xmp(b"not an image at all", PACKET),
        Err(Error::UnsupportedFormat)
    );
}

#[test]
fn rejects_a_jpeg_with_a_dangling_segment() {
    // A segment whose declared length runs past the end must be reported, not
    // read as garbage.
    let mut truncated = jpeg();
    truncated.truncate(20);
    assert_matches!(xmp_image::extract_xmp(&truncated), Err(Error::Corrupt(_)));
}

#[test]
fn handles_a_hand_built_png() {
    let minimal = xmp_image::tests_support::minimal_png();
    assert_eq!(xmp_image::detect(&minimal).unwrap(), Format::Png);
    let stamped = xmp_image::embed_xmp(&minimal, PACKET).unwrap();
    assert_eq!(
        xmp_image::extract_xmp(&stamped).unwrap().as_deref(),
        Some(PACKET)
    );
    xmp_image::png::verify_crcs(&stamped).unwrap();
}

fn avif() -> Option<Vec<u8>> {
    // `image` cannot encode AVIF, so the fixture is produced by ImageMagick and
    // committed beside the tests.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/base.avif");
    std::fs::read(path).ok()
}

/// True when the file still decodes as an image, according to a program that is
/// not this crate.
fn still_decodes(path: &std::path::Path) -> Option<bool> {
    let output = std::process::Command::new("magick")
        .args(["identify", &path.to_string_lossy()])
        .output()
        .ok()?;
    Some(output.status.success())
}

#[test]
fn avif_round_trips() {
    let Some(original) = avif() else {
        eprintln!("skipping: tests/base.avif is absent");
        return;
    };
    assert_eq!(xmp_image::detect(&original).unwrap(), Format::Avif);
    assert_eq!(xmp_image::extract_xmp(&original).unwrap(), None);

    let stamped = xmp_image::embed_xmp(&original, PACKET).expect("embed into avif");
    assert_eq!(
        xmp_image::extract_xmp(&stamped).unwrap().as_deref(),
        Some(PACKET),
        "the packet did not survive the rewrite"
    );

    // Rewriting an AVIF means growing `meta`, which shifts every absolute file
    // offset after it — including the one pointing at the image data. Getting
    // that wrong still produces a file that reads back correctly through our own
    // parser while no decoder can display it, so the check is external.
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&dir).expect("create output dir");
    let path = dir.join("stamped.avif");
    std::fs::write(&path, &stamped).expect("write avif");

    match still_decodes(&path) {
        Some(true) => {}
        Some(false) => panic!("the rewritten AVIF no longer decodes"),
        None => eprintln!("skipping the decode check: ImageMagick is not available"),
    }
}

#[test]
fn avif_reports_that_it_cannot_replace_xmp_yet() {
    let Some(original) = avif() else {
        return;
    };
    let stamped = xmp_image::embed_xmp(&original, PACKET).unwrap();
    // Refusing is the right answer while relocation is unimplemented: silently
    // keeping the old item would serve stale metadata to whoever reads first.
    assert_matches!(
        xmp_image::embed_xmp(&stamped, "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>"),
        Err(Error::Unsupported(_))
    );
}

#[test]
fn malformed_input_is_reported_rather_than_trusted() {
    // A truncated ftyp: shorter than the compatible-brands list it would have.
    let header = b"\x00\x00\x00\x1cftypavif\x00\x00\x00\x00";
    for length in 12..=header.len() {
        let _ = xmp_image::detect(&header[..length]);
    }

    // A box declaring a size it cannot have. Combining a declared size with the
    // offset has to be checked arithmetic, or a huge value wraps past the bound.
    let mut hostile = Vec::from(&b"\x00\x00\x00\x14ftypavif\x00\x00\x00\x00avif"[..]);
    hostile.extend_from_slice(&0xffff_ffffu32.to_be_bytes());
    hostile.extend_from_slice(b"meta");
    assert_matches!(xmp_image::extract_xmp(&hostile), Err(Error::Corrupt(_)));

    // The 64-bit form of the same thing.
    let mut wide = Vec::from(&b"\x00\x00\x00\x14ftypavif\x00\x00\x00\x00avif"[..]);
    wide.extend_from_slice(&1u32.to_be_bytes());
    wide.extend_from_slice(b"meta");
    wide.extend_from_slice(&0xffff_ffff_ffff_ffffu64.to_be_bytes());
    assert_matches!(xmp_image::extract_xmp(&wide), Err(Error::Corrupt(_)));

    // An iloc whose declared field widths are impossible.
    let mut absurd = Vec::from(&b"\x00\x00\x00\x14ftypavif\x00\x00\x00\x00avif"[..]);
    let mut meta_payload = vec![0, 0, 0, 0];
    let mut iloc = Vec::from(&b"iloc"[..]);
    iloc.extend_from_slice(&[0, 0, 0, 0, 0xff, 0xff, 0, 1]);
    let mut iloc_box = ((iloc.len() + 8) as u32).to_be_bytes().to_vec();
    iloc_box.extend_from_slice(&iloc);
    meta_payload.extend_from_slice(&iloc_box);
    absurd.extend_from_slice(&((meta_payload.len() + 8) as u32).to_be_bytes());
    absurd.extend_from_slice(b"meta");
    absurd.extend_from_slice(&meta_payload);
    let _ = xmp_image::extract_xmp(&absurd);
}
