//! The HTTP seam.
//!
//! Everything the signing service does goes through this trait, which is what
//! lets the full c2pa pipeline be tested natively — with a local key standing in
//! for the remote service — instead of only inside a browser.

use crate::error::{Error, Result};
use serde::Deserialize;

/// The `GET /signer` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct SignerInfo {
    pub alg: String,
    #[serde(rename = "coseAlg")]
    pub cose_alg: i64,
    #[serde(rename = "certsPem")]
    pub certs_pem: String,
    /// Bytes to reserve for the manifest. The service is the single source of
    /// truth here: its value tracks its own certificate chain size.
    #[serde(rename = "reserveSize")]
    pub reserve_size: usize,
}

/// Ceilings on what the service may hand back.
///
/// A real chain is three certificates of roughly a kilobyte each. These exist
/// because `certsPem` arrives over the network and would otherwise be decoded
/// into memory without limit.
const MAX_CERTS: usize = 16;
const MAX_CERT_DER_BYTES: usize = 64 * 1024;

impl SignerInfo {
    /// The chain as DER, in the order the service supplied it (leaf first).
    pub fn certs_der(&self) -> Result<Vec<Vec<u8>>> {
        let pems = pem::parse_many(self.certs_pem.as_bytes())
            .map_err(|e| Error::InvalidSignerInfo(format!("certsPem is not valid PEM: {e}")))?;
        if pems.is_empty() {
            return Err(Error::InvalidSignerInfo(
                "certsPem contains no certificates".into(),
            ));
        }
        if pems.len() > MAX_CERTS {
            return Err(Error::InvalidSignerInfo(format!(
                "certsPem carries {} certificates; at most {MAX_CERTS} are accepted",
                pems.len()
            )));
        }
        let mut chain = Vec::with_capacity(pems.len());
        for block in pems {
            let der = block.contents();
            if der.len() > MAX_CERT_DER_BYTES {
                return Err(Error::InvalidSignerInfo(format!(
                    "a certificate is {} bytes; at most {MAX_CERT_DER_BYTES} are accepted",
                    der.len()
                )));
            }
            chain.push(der.to_vec());
        }
        Ok(chain)
    }

    /// `SigningAlg` has no serde impl upstream, so the mapping is explicit here.
    /// Only ES256 is supported, matching what the service advertises.
    pub fn signing_alg(&self) -> Result<c2pa::SigningAlg> {
        match self.alg.to_ascii_lowercase().as_str() {
            "es256" => Ok(c2pa::SigningAlg::Es256),
            other => Err(Error::InvalidSignerInfo(format!(
                "unsupported signer algorithm {other:?}; only es256 is supported"
            ))),
        }
    }
}

// The `?Send` fork is not optional: on wasm the futures are backed by
// `JsFuture`, which is `!Send`, while native transports are `Send`. This mirrors
// how c2pa declares `AsyncSigner` itself.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait SignTransport {
    /// `GET /signer`
    async fn signer_info(&self) -> Result<SignerInfo>;

    /// `POST /sign` — takes the serialized COSE `Sig_structure`, returns the raw
    /// 64-byte `r‖s` signature.
    async fn sign_sig_structure(&self, tbs: Vec<u8>) -> Result<Vec<u8>>;
}

// c2pa's `AsyncSigner` is bounded by its internal `MaybeSend`/`MaybeSync`, which
// are real `Send`/`Sync` on native but relaxed on wasm — where a transport holds
// `JsValue`s that cannot cross threads. The bound therefore has to fork by
// target, or the wasm transport is rejected for not being `Send`.
//
// Not a doc comment: `cfg_select!` expands to the two items, so there is no
// single item for a `///` to attach to.
cfg_select! {
    target_arch = "wasm32" => {
        pub trait TransportBounds: SignTransport {}
        impl<T: SignTransport> TransportBounds for T {}
    }
    _ => {
        pub trait TransportBounds: SignTransport + Send + Sync {}
        impl<T: SignTransport + Send + Sync> TransportBounds for T {}
    }
}
