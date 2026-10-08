//! The object the JS layer actually holds.

use provenance::{RemoteSigner, XmpEdit};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::error::{invalid_argument, redact, to_js_error};
use crate::fetch::FetchTransport;

/// A configured signing session. Built once and reused across images, so that
/// `GET /signer` is fetched once rather than per call.
#[wasm_bindgen]
pub struct Session {
    signer: RemoteSigner<FetchTransport>,
    /// Held only so error messages can be scrubbed of it.
    token: String,
    transport: FetchTransport,
}

impl Session {
    fn js_error(&self, error: provenance::Error) -> JsValue {
        to_js_error(
            error.code().as_str(),
            redact(&error.to_string(), &[&self.token]),
        )
    }
}

/// The fields [`XmpEdit`] accepts, and the fields its `dates` sub-object does.
const EDIT_KEYS: &[&str] = &[
    "basePacket",
    "namespaces",
    "properties",
    "creatorTool",
    "dates",
    "creator",
    "source",
    "rights",
    "webStatement",
    "usageTerms",
    "marked",
];
const DATE_KEYS: &[&str] = &["create", "modify", "metadata"];

fn check_keys(value: &JsValue, allowed: &[&str], context: &str) -> Result<(), JsValue> {
    let object: js_sys::Object = value
        .clone()
        .dyn_into()
        .map_err(|_| invalid_argument(format!("{context} must be an object")))?;
    for key in js_sys::Object::keys(&object).iter() {
        let Some(key) = key.as_string() else { continue };
        if !allowed.contains(&key.as_str()) {
            return Err(invalid_argument(format!(
                "unknown {context} field {key:?}; expected one of {}",
                allowed.join(", ")
            )));
        }
    }
    Ok(())
}

/// Deserialises an edit from JS, rejecting anything unrecognised.
///
/// `serde`'s `deny_unknown_fields` is not honoured by serde-wasm-bindgen's
/// deserializer — it skips unknown keys rather than reporting them — so the
/// check is done here instead. It matters: a caller who mistypes `rights` would
/// otherwise get an image signed without the statement they believed they had
/// written, and no indication that it happened.
pub fn parse_edit(edit: JsValue) -> Result<XmpEdit, JsValue> {
    if edit.is_undefined() || edit.is_null() {
        return Ok(XmpEdit::default());
    }
    check_keys(&edit, EDIT_KEYS, "XMP edit")?;
    if let Ok(dates) = js_sys::Reflect::get(&edit, &JsValue::from_str("dates"))
        && !dates.is_undefined()
        && !dates.is_null()
    {
        check_keys(&dates, DATE_KEYS, "dates")?;
    }

    serde_wasm_bindgen::from_value(edit)
        .map_err(|e| invalid_argument(format!("invalid XMP edit: {e}")))
}

#[wasm_bindgen]
impl Session {
    /// Connects to the signing service and fetches its certificate chain.
    ///
    /// `endpoint` is the service's base URL and `token` its bearer credential.
    /// The token is never compiled in and never written down: it lives only in
    /// this object and in the request headers.
    pub async fn create(
        endpoint: String,
        token: String,
        fetch: JsValue,
    ) -> Result<Session, JsValue> {
        if endpoint.trim().is_empty() {
            return Err(invalid_argument("endpoint must not be empty"));
        }
        if token.trim().is_empty() {
            return Err(invalid_argument("token must not be empty"));
        }

        let transport = FetchTransport::new(endpoint, token.clone());
        let signer = RemoteSigner::new(transport.clone().with_fetch(fetch)?)
            .await
            .map_err(|e| to_js_error(e.code().as_str(), redact(&e.to_string(), &[&token])))?;
        Ok(Session {
            signer,
            token,
            transport,
        })
    }

    /// Writes XMP, then signs the result, returning `{ bytes, format, xmp }`.
    ///
    /// The order is not incidental: signing covers the asset's bytes, so the
    /// metadata written here ends up inside that hash.
    pub async fn process(
        &self,
        image: &[u8],
        edit: JsValue,
        manifest: String,
        title: String,
        fetch: JsValue,
    ) -> Result<JsValue, JsValue> {
        let edit = parse_edit(edit)?;

        let signer = self
            .signer
            .with_transport(self.transport.clone().with_fetch(fetch)?);
        let result = provenance::process(&signer, image, &edit, &manifest, &title)
            .await
            .map_err(|e| self.js_error(e))?;

        let out = js_sys::Object::new();
        let set = |key: &str, value: &JsValue| {
            js_sys::Reflect::set(&out, &JsValue::from_str(key), value)
                .map_err(|_| invalid_argument(format!("could not set {key}")))
        };
        // A `Uint8Array` rather than a serde array: these are whole images, and
        // the array form costs a per-byte conversion.
        set("bytes", &js_sys::Uint8Array::from(result.bytes.as_slice()))?;
        set("format", &JsValue::from_str(&result.format))?;
        set("xmp", &JsValue::from_str(&result.xmp))?;
        Ok(out.into())
    }
}
