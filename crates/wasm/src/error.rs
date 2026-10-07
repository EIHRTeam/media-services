//! Turning Rust failures into JavaScript errors.
//!
//! Two things matter here. The error carries a stable `code`, because callers
//! branch on the kind of failure rather than on a message string. And messages
//! are scrubbed of the bearer token before they leave: `fetch` failures quote
//! the URL, and errors travel into logs and crash reporters.

use wasm_bindgen::JsValue;

/// The token is not secret from the page that supplied it, but it should not
/// end up in a log line or a crash report either.
const REDACTED: &str = "[REDACTED]";

pub fn redact(message: &str, secrets: &[&str]) -> String {
    let mut out = message.to_string();
    for secret in secrets {
        // A one-character token would redact everything, so ignore degenerate
        // values rather than mangling the message.
        if secret.len() < 8 {
            continue;
        }
        out = out.replace(secret, REDACTED);
    }
    out
}

/// Builds a real `Error`, with the classification attached as `code`.
pub fn to_js_error(code: &str, message: String) -> JsValue {
    let error = js_sys::Error::new(&message);
    // Best effort: a failure here would only cost the caller the `code`.
    let _ = js_sys::Reflect::set(&error, &JsValue::from_str("code"), &JsValue::from_str(code));
    error.into()
}

pub fn to_js(error: provenance::Error) -> JsValue {
    to_js_error(error.code().as_str(), error.to_string())
}

pub fn invalid_argument(message: impl Into<String>) -> JsValue {
    to_js_error("invalidArgument", message.into())
}
