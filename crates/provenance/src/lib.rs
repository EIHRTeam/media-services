//! Provenance orchestration: XMP writing followed by C2PA signing.
//!
//! The HTTP the signing service speaks sits behind [`SignTransport`], which is
//! what lets the whole pipeline run natively — with a local key standing in for
//! the remote service — instead of only inside a browser.
//!
//! ```no_run
//! use provenance::{process, RemoteSigner, TransportBounds, XmpEdit};
//!
//! # async fn run<T: TransportBounds + 'static>(
//! #     transport: T,
//! #     image: &[u8],
//! #     manifest_json: &str,
//! # ) -> Result<(), provenance::Error> {
//! // Built once and reused: this is what fetches `GET /signer`.
//! let signer = RemoteSigner::new(transport).await?;
//!
//! let edit = XmpEdit {
//!     creator_tool: Some("EIHRTeam Media Provenance Service".into()),
//!     ..Default::default()
//! };
//! let result = process(&signer, image, &edit, manifest_json, "photo.png").await?;
//! println!("{} bytes of {}", result.bytes.len(), result.format);
//! # Ok(())
//! # }
//! ```

pub mod edit;
pub mod error;
pub mod pipeline;
pub mod signer;
pub mod transport;

pub use edit::{Dates, XmpEdit};
pub use error::{Error, ErrorCode, Result};
pub use pipeline::{Processed, process, read_xmp, write_xmp};
pub use signer::RemoteSigner;
pub use transport::{SignTransport, SignerInfo, TransportBounds};

/// Phase 0 size probe: exercises the real builder path so a wasm artifact
/// actually contains `c2pa`. Referencing a type alone lets the linker strip
/// nearly everything and report a meaningless size.
#[doc(hidden)]
pub fn probe_build(manifest_json: &str) -> std::result::Result<usize, String> {
    let _builder = c2pa::Builder::from_context(c2pa::Context::new())
        .with_definition(manifest_json)
        .map_err(|e| e.to_string())?;
    Ok(c2pa::Builder::supported_mime_types().len())
}
