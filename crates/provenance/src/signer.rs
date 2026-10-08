//! Bridges the remote signing service into c2pa's `AsyncSigner`.

use crate::error::Result;
use crate::transport::{SignTransport, SignerInfo, TransportBounds};

/// Ceiling on the size the service may ask us to reserve.
///
/// The deployed chain needs ~12 KiB and a manifest heavy with assertions might
/// need a few hundred, so this is far above any legitimate value. It exists
/// because `reserveSize` arrives from the network and c2pa allocates against it:
/// an unbounded value would let a broken or hostile service force an allocation
/// of that size, inside a browser tab.
const MAX_RESERVE_SIZE: usize = 1024 * 1024;

/// The signing service, seen through c2pa's `AsyncSigner` trait.
///
/// `direct_cose_handling` is deliberately left at its default (`false`). With it
/// false, c2pa hands us the serialized COSE `Sig_structure` and expects the bare
/// signature back — exactly the contract `POST /sign` implements. Returning
/// `true` instead would make us responsible for assembling the `COSE_Sign1`
/// ourselves, which nothing here needs.
pub struct RemoteSigner<T> {
    transport: T,
    info: SignerInfo,
    alg: c2pa::SigningAlg,
    certs_der: Vec<Vec<u8>>,
}

impl<T: SignTransport> RemoteSigner<T> {
    /// Fetches signer metadata once. Reusing the instance across images avoids
    /// re-hitting `GET /signer` for every signature.
    pub async fn new(transport: T) -> Result<Self> {
        let info = transport.signer_info().await?;
        if info.reserve_size == 0 || info.reserve_size > MAX_RESERVE_SIZE {
            return Err(crate::error::Error::InvalidSignerInfo(format!(
                "reserveSize {} is outside the supported range 1..={MAX_RESERVE_SIZE}",
                info.reserve_size
            )));
        }
        let alg = info.signing_alg()?;
        let certs_der = info.certs_der()?;
        Ok(Self {
            transport,
            info,
            alg,
            certs_der,
        })
    }

    /// Reuses validated signing identity with a per-operation transport.
    pub fn with_transport<U>(&self, transport: U) -> RemoteSigner<U> {
        RemoteSigner {
            transport,
            info: self.info.clone(),
            alg: self.alg,
            certs_der: self.certs_der.clone(),
        }
    }

    pub fn info(&self) -> &SignerInfo {
        &self.info
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl<T: TransportBounds> c2pa::AsyncSigner for RemoteSigner<T> {
    async fn sign(&self, data: Vec<u8>) -> c2pa::Result<Vec<u8>> {
        // `data` is the Sig_structure; the service returns raw r‖s.
        let signature = self.transport.sign_sig_structure(data).await?;
        Ok(signature)
    }

    fn alg(&self) -> c2pa::SigningAlg {
        self.alg
    }

    fn certs(&self) -> c2pa::Result<Vec<Vec<u8>>> {
        // Must match the service's own leaf, or it rejects the x5chain.
        Ok(self.certs_der.clone())
    }

    fn reserve_size(&self) -> usize {
        self.info.reserve_size
    }
}
