//! WebAssembly bindings.
//!
//! This layer only converts types. The pipeline, the signing protocol and the
//! container handling all live in `provenance`, where they can be tested without
//! a browser — which is the whole reason the HTTP call sits behind a trait.
//!
//! Everything here is gated on `wasm32`: the browser transport needs `web-sys`,
//! which is not a dependency elsewhere, and an empty crate on other targets
//! keeps `cargo test --workspace` working on the host.

#[cfg(target_arch = "wasm32")]
mod error;
#[cfg(target_arch = "wasm32")]
mod fetch;
#[cfg(target_arch = "wasm32")]
mod session;

#[cfg(target_arch = "wasm32")]
pub use session::Session;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Installs a panic hook that reports through `console.error`.
///
/// Without it a panic in wasm surfaces as an opaque `unreachable` with no
/// message, which makes debugging in a browser essentially impossible.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// The container MIME type of `image`, or an error if it is not one we handle.
///
/// Exposed so a caller can reject a file before reading it into memory.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = detectFormat)]
pub fn detect_format(image: &[u8]) -> Result<String, JsValue> {
    xmp_image::detect(image)
        .map(|format| format.mime().to_string())
        .map_err(|e| error::to_js(provenance::Error::from(e)))
}

/// The XMP packet an image carries, or `undefined` if it has none.
///
/// Free functions rather than methods on `Session`: reading metadata is not a
/// privileged operation, and requiring a signing credential for it would be a
/// gratuitous coupling.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = readXmp)]
pub fn read_xmp(image: &[u8]) -> Result<Option<String>, JsValue> {
    provenance::read_xmp(image).map_err(error::to_js)
}

/// Writes XMP without signing. Returns the rewritten image.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = writeXmp)]
pub fn write_xmp(image: &[u8], edit: JsValue) -> Result<Vec<u8>, JsValue> {
    let edit = session::parse_edit(edit)?;
    let (bytes, _packet) = provenance::write_xmp(image, &edit).map_err(error::to_js)?;
    Ok(bytes)
}
