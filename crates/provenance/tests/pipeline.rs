//! The full pipeline: XMP written into the image, then the result signed.
//!
//! The assertion that matters is not that both steps ran, but that the metadata
//! ended up *inside* the signature — changing a character of it afterwards has
//! to invalidate the manifest, or stamping metadata and signing the asset would
//! be two unrelated operations.

mod support;

use provenance::{RemoteSigner, XmpEdit, process};
use support::{LocalKeyTransport, load_or_generate_chain};

fn edit() -> XmpEdit {
    XmpEdit {
        creator_tool: Some("EIHRTeam Media Provenance Service".into()),
        dates: Some(provenance::Dates {
            create: Some("2026-10-07T04:03:52+09:00".into()),
            modify: Some("2026-10-07T04:03:52+09:00".into()),
            metadata: Some("2026-10-07T04:03:52+09:00".into()),
        }),
        creator: Some(vec!["The SKLAND Endfield Wiki Editorial Team".into()]),
        source: Some("Shanghai Hypergryph Network Technology Co., Ltd.".into()),
        rights: Some(
            [(
                "x-default".to_string(),
                "Copyright © Shanghai Hypergryph Network Technology Co., Ltd. All Rights Reserved."
                    .to_string(),
            )]
            .into_iter()
            .collect(),
        ),
        web_statement: Some("https://endfield.gryphline.com/en-us/news/4497".into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn stamps_xmp_then_signs_the_stamped_bytes() {
    let image = support::test_jpeg();
    let chain = load_or_generate_chain();
    let signer = RemoteSigner::new(LocalKeyTransport::new(&chain))
        .await
        .expect("build signer");

    // The asset must start clean, or the test would not prove the XMP came from
    // this run.
    assert_eq!(provenance::read_xmp(&image).unwrap(), None);

    let result = process(
        &signer,
        &image,
        &edit(),
        &support::manifest_json("probe.jpg"),
        "probe.jpg",
    )
    .await
    .expect("process");

    assert_eq!(result.format, "image/jpeg");

    // The metadata is in the file...
    let stamped = provenance::read_xmp(&result.bytes).unwrap().expect("xmp");
    assert!(
        stamped.contains("EIHRTeam Media Provenance Service"),
        "{stamped}"
    );
    assert!(
        stamped.contains("The SKLAND Endfield Wiki Editorial Team"),
        "{stamped}"
    );

    // ...and `dc:format` describes this asset rather than the template's guess.
    let parsed = xmp::Xmp::parse(&stamped).unwrap();
    assert_eq!(parsed.get_text("dc", "format"), Some("image/jpeg"));
    // The template's own timestamps must be gone.
    assert_eq!(
        parsed.get_text("xmp", "CreateDate"),
        Some("2026-10-07T04:03:52+09:00")
    );

    support::assert_signature_and_hash_valid(&result.bytes, "image/jpeg");

    let out_dir = support::output_dir();
    std::fs::write(support::signed_dir().join("stamped.jpg"), &result.bytes)
        .expect("write artifact");
    // Also a plain image, so the Node integration test has an input that has
    // never been near the Rust side.
    std::fs::write(out_dir.join("plain.jpg"), support::test_jpeg()).expect("write plain jpeg");
    if let Some(root) = &chain.root_pem {
        std::fs::write(out_dir.join("root-ca.pem"), root).expect("write root ca");
    }
    support::write_test_identity(&chain);
}

#[tokio::test]
async fn editing_the_metadata_afterwards_invalidates_the_manifest() {
    let chain = load_or_generate_chain();
    let signer = RemoteSigner::new(LocalKeyTransport::new(&chain))
        .await
        .expect("build signer");

    let result = process(
        &signer,
        &support::test_jpeg(),
        &edit(),
        &support::manifest_json("probe.jpg"),
        "probe.jpg",
    )
    .await
    .expect("process");

    support::assert_signature_and_hash_valid(&result.bytes, "image/jpeg");

    // Flip one byte inside the *XMP packet*, leaving its length and position
    // untouched so nothing but the bytes changes.
    //
    // Locating it by searching for a metadata string would be wrong: the same
    // text also appears inside the manifest, and the manifest is exactly the
    // region the asset hash excludes — so corrupting that instead would leave
    // the hash matching and prove nothing.
    let mut tampered = result.bytes.clone();
    let header = b"http://ns.adobe.com/xap/1.0/\0";
    let segment = tampered
        .windows(header.len())
        .position(|w| w == header)
        .expect("the stamped XMP segment should be findable");
    // Well past the `<?xpacket?>` prologue, so this lands in the packet body.
    let target = segment + header.len() + 80;
    tampered[target] ^= 0x01;

    let reader = c2pa::Reader::from_context(c2pa::Context::new())
        .with_stream("image/jpeg", std::io::Cursor::new(tampered))
        .expect("read back");
    let rendered = serde_json::to_string(reader.validation_results().unwrap()).unwrap();
    assert!(
        rendered.contains("dataHash.mismatch"),
        "editing the metadata after signing should have broken the asset hash: {rendered}"
    );
}

#[tokio::test]
async fn a_second_edit_merges_rather_than_replaces() {
    let chain = load_or_generate_chain();
    let signer = RemoteSigner::new(LocalKeyTransport::new(&chain))
        .await
        .expect("build signer");

    let first = process(
        &signer,
        &support::test_jpeg(),
        &edit(),
        &support::manifest_json("probe.jpg"),
        "probe.jpg",
    )
    .await
    .expect("process");

    // A later pass that only changes the tool must leave the rights fields the
    // first pass wrote, or every re-run would wipe the previous metadata.
    let followup = XmpEdit {
        creator_tool: Some("Another Tool".into()),
        ..Default::default()
    };
    let carried = provenance::read_xmp(&first.bytes).unwrap().expect("xmp");
    let xmp = followup.apply(Some(&carried)).unwrap();

    assert_eq!(xmp.get_text("xmp", "CreatorTool"), Some("Another Tool"));
    assert_eq!(
        xmp.get_text("dc", "source"),
        Some("Shanghai Hypergryph Network Technology Co., Ltd."),
        "an unrelated field was dropped"
    );
    assert_eq!(
        xmp.get_text("xmp", "CreateDate"),
        Some("2026-10-07T04:03:52+09:00"),
        "the timestamps from the first pass were lost"
    );
}

#[tokio::test]
async fn every_timestamp_is_the_moment_of_processing() {
    let chain = load_or_generate_chain();
    let signer = RemoteSigner::new(LocalKeyTransport::new(&chain))
        .await
        .expect("build signer");

    let stamp = "2026-10-07T19:30:00+09:00";
    let mut edit = edit();
    edit.dates = Some(provenance::Dates {
        create: Some(stamp.into()),
        modify: Some(stamp.into()),
        metadata: Some(stamp.into()),
    });

    let result = process(
        &signer,
        &support::test_jpeg(),
        &edit,
        &support::manifest_json("probe.jpg"),
        "probe.jpg",
    )
    .await
    .expect("process");

    // The XMP side.
    let xmp = xmp::Xmp::parse(&provenance::read_xmp(&result.bytes).unwrap().unwrap()).unwrap();
    for field in ["CreateDate", "ModifyDate", "MetadataDate"] {
        assert_eq!(xmp.get_text("xmp", field), Some(stamp), "{field}");
    }

    // And the C2PA side, read back out of the signed asset: an action with no
    // `when` says nothing about when it happened.
    let reader = c2pa::Reader::from_context(c2pa::Context::new())
        .with_stream("image/jpeg", std::io::Cursor::new(result.bytes.clone()))
        .expect("read back");
    let rendered = reader.json();
    let manifest: serde_json::Value = serde_json::from_str(&rendered).expect("reader json");
    let actions = manifest["manifests"]
        .as_object()
        .expect("manifests")
        .values()
        .flat_map(|entry| entry["assertions"].as_array().into_iter().flatten())
        .find(|assertion| {
            assertion["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("c2pa.actions"))
        })
        .and_then(|assertion| assertion["data"]["actions"].as_array().cloned())
        .expect("actions assertion");

    assert!(!actions.is_empty());
    for action in &actions {
        assert_eq!(
            action["when"].as_str(),
            Some(stamp),
            "action {:?} carries no processing time",
            action["action"]
        );
    }
}

#[test]
fn a_template_that_pins_an_action_time_keeps_it() {
    // Filling in a missing value is helpful; overwriting a deliberate one is not.
    let mut manifest: serde_json::Value =
        serde_json::from_str(&support::manifest_json("probe.jpg")).unwrap();
    manifest["assertions"][0]["data"]["actions"][0]["when"] =
        serde_json::Value::String("2020-01-01T00:00:00Z".into());

    let prepared = provenance::pipeline::prepare_manifest_for_test(
        &manifest.to_string(),
        "probe.jpg",
        "image/jpeg",
        Some("2026-10-07T19:30:00+09:00"),
    )
    .expect("prepare");
    let prepared: serde_json::Value = serde_json::from_str(&prepared).unwrap();
    let actions = prepared["assertions"][0]["data"]["actions"]
        .as_array()
        .unwrap();

    assert_eq!(actions[0]["when"].as_str(), Some("2020-01-01T00:00:00Z"));
    assert_eq!(
        actions[1]["when"].as_str(),
        Some("2026-10-07T19:30:00+09:00")
    );
}

/// The timestamp contract, stated as a test because "do I blank the fields out
/// first?" is exactly the question a caller has to ask.
#[test]
fn a_base_packet_keeps_its_own_dates_unless_dates_are_given() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../xmp/hypergryph.xml");
    let base = std::fs::read_to_string(path).expect("read the shipped template");

    // Nothing supplied means nothing invented: the template's own values stand.
    let untouched = XmpEdit {
        base_packet: Some(base.clone()),
        ..Default::default()
    }
    .apply(None)
    .expect("apply");
    assert_eq!(
        untouched.get_text("xmp", "CreateDate"),
        Some("2026-01-22T12:00:00+08:00")
    );

    // Supplied means replaced in place, without the caller blanking anything.
    let stamped = XmpEdit {
        base_packet: Some(base),
        dates: Some(provenance::Dates {
            create: Some("2026-10-07T19:30:00+08:00".into()),
            modify: Some("2026-10-07T19:30:00+08:00".into()),
            metadata: Some("2026-10-07T19:30:00+08:00".into()),
        }),
        ..Default::default()
    }
    .apply(None)
    .expect("apply");

    for field in ["CreateDate", "ModifyDate", "MetadataDate"] {
        assert_eq!(
            stamped.get_text("xmp", field),
            Some("2026-10-07T19:30:00+08:00"),
            "{field} was left as the template's own value"
        );
    }
    // And the rest of the template survives untouched.
    assert_eq!(
        stamped.get_text("photoshop", "Source"),
        Some("Shanghai Hypergryph Network Technology Co., Ltd.")
    );
    assert_eq!(
        stamped.get_text("xmp", "CreatorTool"),
        Some("EIHRTeam Media Provenance Service")
    );
}

#[tokio::test]
async fn webp_and_avif_rewrites_keep_valid_signature_and_asset_hash() {
    let chain = load_or_generate_chain();
    let signer = RemoteSigner::new(LocalKeyTransport::new(&chain))
        .await
        .unwrap();
    let avif = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../xmp-image/tests/base.avif"),
    )
    .unwrap();
    for (input, mime, title) in [
        (
            support::test_image(image::ImageFormat::WebP),
            "image/webp",
            "probe.webp",
        ),
        (avif, "image/avif", "probe.avif"),
    ] {
        let result = process(
            &signer,
            &input,
            &edit(),
            &support::manifest_json(title),
            title,
        )
        .await
        .unwrap();
        assert_eq!(result.format, mime);
        assert!(
            provenance::read_xmp(&result.bytes)
                .unwrap()
                .unwrap()
                .contains("EIHRTeam Media Provenance Service")
        );
        support::assert_signature_and_hash_valid(&result.bytes, mime);
        std::fs::write(
            support::signed_dir()
                .join(format!("stamped.{}", title.split('.').next_back().unwrap())),
            result.bytes,
        )
        .unwrap();
        if mime == "image/webp" {
            std::fs::write(support::output_dir().join("plain.webp"), input).unwrap();
        }
    }
}
