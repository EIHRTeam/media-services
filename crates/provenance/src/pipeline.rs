//! The ordered pipeline: write XMP into the image, then sign the result.
//!
//! The order is the whole point. Signing covers the asset's bytes with a
//! hard-binding hash, so writing XMP first puts it inside that hash — change a
//! character of the metadata afterwards and the manifest stops validating. The
//! reverse order would leave the XMP signing with no provenance at all.

use std::io::Cursor;

use crate::edit::XmpEdit;
use crate::error::{Error, Result};
use crate::signer::RemoteSigner;
use crate::transport::TransportBounds;

/// A signed image and the metadata that went into it.
pub struct Processed {
    pub bytes: Vec<u8>,
    /// The container's MIME type, detected rather than assumed.
    pub format: String,
    /// The XMP packet as written, so callers can show what was stamped.
    pub xmp: String,
}

/// Reads the XMP packet an image carries, if any.
pub fn read_xmp(image: &[u8]) -> Result<Option<String>> {
    Ok(xmp_image::extract_xmp(image)?)
}

/// Writes XMP without signing. Returns the rewritten image and the packet.
pub fn write_xmp(image: &[u8], edit: &XmpEdit) -> Result<(Vec<u8>, String)> {
    let format = xmp_image::detect(image)?;
    let existing = xmp_image::extract_xmp_as(image, format)?;
    let xmp = edit.apply(existing.as_deref())?;
    let packet = xmp.to_packet();
    let stamped = xmp_image::embed_xmp_as(image, format, &packet)?;
    Ok((stamped, packet))
}

/// Writes XMP, then signs the result.
///
/// `manifest_json` is the c2patool manifest format, so the templates under
/// `c2pa/manifest/` can be passed through unchanged apart from `title` and
/// `format`, which are derived from the asset being signed.
///
/// The signer is a parameter rather than something built here, so a caller
/// stamping many images hits `GET /signer` once instead of once per image.
pub async fn process<T: TransportBounds>(
    signer: &RemoteSigner<T>,
    image: &[u8],
    edit: &XmpEdit,
    manifest_json: &str,
    title: &str,
) -> Result<Processed> {
    let format = xmp_image::detect(image)?;
    let mime = format.mime();

    // Start from whatever XMP the image already carries, so a re-run updates
    // rather than replaces.
    let existing = xmp_image::extract_xmp_as(image, format)?;
    let mut xmp = edit.apply(existing.as_deref())?;
    // `dc:format` has to describe this asset. The templates hardcode image/png,
    // which would be a lie for a JPEG.
    xmp.set_text("dc", "format", mime)
        .map_err(|e| Error::MalformedXmp(e.to_string()))?;

    let packet = xmp.to_packet();
    let stamped = xmp_image::embed_xmp_as(image, format, &packet)?;

    // The manifest records when each action happened. Taking that from the same
    // value that went into the XMP keeps the two from disagreeing about when
    // this asset was stamped.
    let when = edit.dates.as_ref().and_then(crate::edit::Dates::stamp);
    let manifest = prepare_manifest(manifest_json, title, mime, when)?;
    let signed = sign(signer, &stamped, mime, &manifest).await?;

    Ok(Processed {
        bytes: signed,
        format: mime.to_string(),
        xmp: packet,
    })
}

fn prepare_manifest(
    manifest_json: &str,
    title: &str,
    mime: &str,
    when: Option<&str>,
) -> Result<String> {
    let mut definition: serde_json::Value =
        serde_json::from_str(manifest_json).map_err(|e| Error::MalformedManifest(e.to_string()))?;
    let fields = definition
        .as_object_mut()
        .ok_or_else(|| Error::MalformedManifest("manifest is not a JSON object".into()))?;
    fields.insert("title".into(), title.into());
    fields.insert("format".into(), mime.into());

    if let Some(when) = when {
        stamp_actions(&mut definition, when);
    }
    serde_json::to_string(&definition).map_err(|e| Error::MalformedManifest(e.to_string()))
}

/// Writes `when` onto every action that does not already carry one.
///
/// A value already in the template wins: a caller who pinned an action's time
/// meant it, and overwriting that would be the wrong kind of helpful.
fn stamp_actions(definition: &mut serde_json::Value, when: &str) {
    let Some(assertions) = definition
        .get_mut("assertions")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for assertion in assertions {
        let label = assertion
            .get("label")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !label.starts_with("c2pa.actions") {
            continue;
        }
        let Some(actions) = assertion
            .get_mut("data")
            .and_then(|data| data.get_mut("actions"))
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for action in actions {
            let Some(object) = action.as_object_mut() else {
                continue;
            };
            object
                .entry("when")
                .or_insert_with(|| serde_json::Value::String(when.to_string()));
        }
    }
}

async fn sign<T: TransportBounds>(
    signer: &RemoteSigner<T>,
    image: &[u8],
    mime: &str,
    manifest_json: &str,
) -> Result<Vec<u8>> {
    // Signing does not self-verify. With a private CA the trust check can never
    // pass, so leaving it on would fail every real deployment; a validator is
    // the right place for that judgement.
    let settings = serde_json::json!({ "verify": { "verify_after_sign": false } }).to_string();
    let context = c2pa::Context::new()
        .with_settings(settings.as_str())
        .map_err(Error::C2pa)?;

    let mut builder = c2pa::Builder::from_context(context)
        .with_definition(manifest_json)
        .map_err(|e| Error::MalformedManifest(e.to_string()))?;

    let mut source = Cursor::new(image);
    let mut dest = Cursor::new(Vec::new());
    builder
        .sign_async(signer, mime, &mut source, &mut dest)
        .await?;
    Ok(dest.into_inner())
}

/// Exposes manifest preparation so the timestamp rules can be tested without a
/// signing round trip.
#[doc(hidden)]
pub fn prepare_manifest_for_test(
    manifest_json: &str,
    title: &str,
    mime: &str,
    when: Option<&str>,
) -> Result<String> {
    prepare_manifest(manifest_json, title, mime, when)
}
