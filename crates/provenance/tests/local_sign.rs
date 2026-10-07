//! Signing a real JPEG through c2pa's async pipeline, with a local P-256 key
//! standing in for the remote signing service.
//!
//! This is the native counterpart to the browser flow. Because the HTTP call
//! sits behind `SignTransport`, the whole pipeline runs with no network, no
//! browser and no wasm — which is the point of the trait.

mod support;

use std::io::Cursor;

use provenance::RemoteSigner;
use support::{LocalKeyTransport, load_or_generate_chain, test_jpeg};

#[tokio::test]
async fn signs_a_jpeg_through_the_async_pipeline() {
    let chain = load_or_generate_chain();
    let transport = LocalKeyTransport::new(&chain);
    let captured = transport.handle();
    let expected_reserve = transport.info.reserve_size;

    let signer = RemoteSigner::new(transport).await.expect("build signer");
    // `certs()` feeds the COSE x5chain, and the service byte-compares it against
    // its own leaf — so the chain must survive the PEM round trip intact.
    assert_eq!(signer.info().reserve_size, expected_reserve);
    {
        use c2pa::AsyncSigner as _;
        assert_eq!(
            signer.certs().expect("certs"),
            chain.chain_der,
            "certs() must return exactly the chain, in order"
        );
    }

    // The signing path deliberately does not self-verify: with a private CA,
    // post-sign trust verification can never pass, so leaving it on would make
    // `sign_async` fail for every real deployment. Verification belongs to a
    // validator — `scripts/acceptance.mjs` runs c2patool against this artifact.
    // The cost is that content-level bugs are not caught here.
    let settings = serde_json::json!({ "verify": { "verify_after_sign": false } }).to_string();
    let context = c2pa::Context::new()
        .with_settings(settings.as_str())
        .expect("settings");
    let mut builder = c2pa::Builder::from_context(context)
        .with_definition(support::manifest_json("probe.jpg"))
        .expect("parse manifest");

    let mut source = Cursor::new(test_jpeg());
    let mut dest = Cursor::new(Vec::new());
    builder
        .sign_async(&signer, "image/jpeg", &mut source, &mut dest)
        .await
        .expect("sign");

    let signed = dest.into_inner();
    assert_eq!(&signed[..2], &[0xFF, 0xD8], "output is not a JPEG");

    let captured = captured.lock().expect("capture lock");

    support::assert_signature_and_hash_valid(&signed, "image/jpeg");

    // The load-bearing assumption of the whole design: c2pa hands the signer a
    // serialized COSE Sig_structure rather than raw claim bytes, so the service's
    // `POST /sign` contract fits with no COSE assembly on our side. CBOR for
    // `["Signature1", protected, aad, payload]` starts 0x84 (array of four) then
    // 0x6a (text of length ten).
    assert!(
        captured.tbs.len() > 12 && captured.tbs[..12] == *b"\x84\x6aSignature1",
        "signer did not receive a COSE Sig_structure; got {:02x?}",
        &captured.tbs[..captured.tbs.len().min(16)]
    );

    // And the bytes we returned really are a valid ES256 signature over that
    // payload — the property that makes the service's raw `r‖s` reply usable.
    use p256::ecdsa::signature::Verifier as _;
    let signature = p256::ecdsa::Signature::from_slice(&captured.signature).expect("64-byte r‖s");
    p256::ecdsa::VerifyingKey::from(&signer_verifying_key(&chain))
        .verify(&captured.tbs, &signature)
        .expect("signature must verify over the Sig_structure c2pa provided");

    // Hand the artifact and its trust anchor to the c2patool acceptance step.
    let out_dir = support::signed_dir();
    std::fs::write(out_dir.join("signed.jpg"), &signed).expect("write signed jpeg");
    if let Some(root) = &chain.root_pem {
        std::fs::write(out_dir.join("root-ca.pem"), root).expect("write root ca");
    }
    eprintln!(
        "wrote {} ({} bytes); Sig_structure was {} bytes",
        out_dir.join("signed.jpg").display(),
        signed.len(),
        captured.tbs.len()
    );
}

fn signer_verifying_key(chain: &support::TestChain) -> p256::ecdsa::SigningKey {
    use p256::pkcs8::DecodePrivateKey as _;
    p256::ecdsa::SigningKey::from_pkcs8_pem(chain.key_pem.trim()).expect("PKCS#8 P-256")
}
