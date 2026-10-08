//! Shared test scaffolding.
//!
//! The signing identity is generated per run rather than committed, so the suite
//! depends on no external repository and no private key lands in git. Setting
//! `C2PA_TEST_KEY_PEM` and `C2PA_TEST_CHAIN_PEM` swaps in a real chain instead,
//! which is how an artifact can be checked against a deployed certificate
//! without this crate hardcoding a path into another repository.

// `expect` rather than `allow`: this module is compiled once per test target and
// each one uses a different slice of it, so "dead" here means "unused by this
// target". If a target ever did use all of it, the expectation going unfulfilled
// is the signal to drop this attribute rather than leave it lying.
#![expect(dead_code)]

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use provenance::{Error, SignTransport, SignerInfo};

/// A throwaway signing identity: one P-256 key and a three-cert chain.
pub struct TestChain {
    pub key_pem: String,
    /// Leaf first, then issuer — the order `x5chain` expects.
    pub chain_pem: String,
    pub chain_der: Vec<Vec<u8>>,
    /// The root, when we generated it ourselves; supplied chains have none.
    pub root_pem: Option<String>,
}

pub fn load_or_generate_chain() -> TestChain {
    let (Ok(key_path), Ok(chain_path)) = (
        std::env::var("C2PA_TEST_KEY_PEM"),
        std::env::var("C2PA_TEST_CHAIN_PEM"),
    ) else {
        return generate_chain();
    };

    let key_pem =
        std::fs::read_to_string(&key_path).unwrap_or_else(|e| panic!("read {key_path}: {e}"));
    let chain_pem =
        std::fs::read_to_string(&chain_path).unwrap_or_else(|e| panic!("read {chain_path}: {e}"));
    let chain_der: Vec<Vec<u8>> = pem::parse_many(chain_pem.as_bytes())
        .expect("chain PEM does not parse")
        .into_iter()
        .map(|p| p.contents().to_vec())
        .collect();
    eprintln!(
        "using supplied chain from {chain_path} ({} certs)",
        chain_der.len()
    );

    TestChain {
        key_pem,
        chain_pem,
        chain_der,
        root_pem: None,
    }
}

fn generate_chain() -> TestChain {
    let alg = &rcgen::PKCS_ECDSA_P256_SHA256;

    let root_key = rcgen::KeyPair::generate_for(alg).expect("generate root key");
    let mut root_params = rcgen::CertificateParams::new(Vec::new()).expect("root params");
    root_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    root_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "media-services test root");
    let root_cert = root_params.self_signed(&root_key).expect("self-sign root");

    // Mirror the deployed chain's shape: root -> intermediate -> leaf.
    let intermediate_key = rcgen::KeyPair::generate_for(alg).expect("generate intermediate key");
    let mut intermediate_params =
        rcgen::CertificateParams::new(Vec::new()).expect("intermediate params");
    intermediate_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
    intermediate_params.distinguished_name.push(
        rcgen::DnType::CommonName,
        "media-services test intermediate",
    );
    intermediate_params.use_authority_key_identifier_extension = true;
    let intermediate_cert = intermediate_params
        .signed_by(&intermediate_key, &root_cert, &root_key)
        .expect("sign intermediate");

    let leaf_key = rcgen::KeyPair::generate_for(alg).expect("generate leaf key");
    let mut leaf_params = rcgen::CertificateParams::new(Vec::new()).expect("leaf params");
    leaf_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "media-services test signer");
    // c2pa validates the signing certificate's profile *during signing*, whether
    // or not post-sign verification is enabled, so a chain without these is
    // rejected with CertificateProfileError. These are the same properties the
    // deployed signing certificate carries; rcgen omits most of them by default.
    leaf_params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    leaf_params.is_ca = rcgen::IsCa::ExplicitNoCa;
    leaf_params.use_authority_key_identifier_extension = true;
    leaf_params.extended_key_usages = vec![
        rcgen::ExtendedKeyUsagePurpose::EmailProtection,
        // id-kp-documentSigning
        rcgen::ExtendedKeyUsagePurpose::Other(vec![1, 3, 6, 1, 5, 5, 7, 3, 36]),
        // C2PA private EKU
        rcgen::ExtendedKeyUsagePurpose::Other(vec![1, 3, 6, 1, 4, 1, 62558, 2, 1]),
    ];
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &intermediate_cert, &intermediate_key)
        .expect("sign leaf");

    TestChain {
        key_pem: leaf_key.serialize_pem(),
        chain_pem: format!(
            "{}{}{}",
            leaf_cert.pem(),
            intermediate_cert.pem(),
            root_cert.pem()
        ),
        chain_der: vec![
            leaf_cert.der().to_vec(),
            intermediate_cert.der().to_vec(),
            root_cert.der().to_vec(),
        ],
        root_pem: Some(root_cert.pem()),
    }
}

/// Records what c2pa handed to the signer, so tests can assert both the shape of
/// the payload and that the signature is self-consistent.
#[derive(Default)]
pub struct Captured {
    pub tbs: Vec<u8>,
    pub signature: Vec<u8>,
    pub calls: usize,
}

/// Signs the Sig_structure the way the service does: ES256 over the given bytes,
/// returning raw `r‖s` rather than DER.
pub struct LocalKeyTransport {
    key: p256::ecdsa::SigningKey,
    pub info: SignerInfo,
    pub captured: Arc<Mutex<Captured>>,
}

impl LocalKeyTransport {
    pub fn new(chain: &TestChain) -> Self {
        use p256::pkcs8::DecodePrivateKey as _;
        let key = p256::ecdsa::SigningKey::from_pkcs8_pem(chain.key_pem.trim())
            .expect("generated key is not PKCS#8 P-256");
        Self {
            key,
            info: SignerInfo {
                alg: "es256".into(),
                cose_alg: -7,
                certs_pem: chain.chain_pem.clone(),
                reserve_size: reserve_size_for(&chain.chain_der),
            },
            captured: Arc::new(Mutex::new(Captured::default())),
        }
    }

    pub fn handle(&self) -> Arc<Mutex<Captured>> {
        Arc::clone(&self.captured)
    }
}

#[async_trait::async_trait]
impl SignTransport for LocalKeyTransport {
    async fn signer_info(&self) -> Result<SignerInfo, Error> {
        Ok(self.info.clone())
    }

    async fn sign_sig_structure(&self, tbs: Vec<u8>) -> Result<Vec<u8>, Error> {
        use p256::ecdsa::signature::Signer as _;
        // `SigningKey::sign` hashes with SHA-256 first, which is what COSE ES256
        // requires.
        let signature: p256::ecdsa::Signature = self.key.sign(&tbs);
        let bytes = signature.to_bytes();
        assert_eq!(bytes.len(), 64, "ES256 signatures are raw r‖s, 64 bytes");

        let mut captured = self.captured.lock().expect("capture lock");
        captured.tbs = tbs;
        captured.signature = bytes.to_vec();
        captured.calls += 1;
        Ok(bytes.to_vec())
    }
}

/// The service's own formula: a fixed overhead plus the encoded chain length.
pub fn reserve_size_for(chain_der: &[Vec<u8>]) -> usize {
    10_000 + chain_der.iter().map(Vec::len).sum::<usize>()
}

pub fn test_image(format: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbImage::from_fn(48, 48, |x, y| {
        image::Rgb([(x * 5) as u8, (y * 5) as u8, 128])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, format)
        .expect("encode test image");
    out.into_inner()
}

pub fn test_jpeg() -> Vec<u8> {
    test_image(image::ImageFormat::Jpeg)
}

/// Loads the repo's real manifest template and pins title/format to the asset
/// actually being signed. Testing against the production template — rather than
/// a hand-written stub — is the point: it is the artifact that ships.
pub fn manifest_json(title: &str) -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../c2pa/manifest/hypergryph.json");
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut def: serde_json::Value = serde_json::from_str(&raw).expect("template is not JSON");
    def["title"] = serde_json::Value::String(title.to_string());
    def["format"] = serde_json::Value::String("image/jpeg".into());
    serde_json::to_string(&def).expect("reserialize manifest")
}

/// Where artifacts are written for the c2patool acceptance step.
pub fn output_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).expect("create output dir");
    dir
}

/// Where signed artifacts go, kept apart from the inputs and fixtures so the
/// acceptance step does not try to verify a file that was never meant to carry
/// a manifest.
pub fn signed_dir() -> PathBuf {
    let dir = output_dir().join("signed");
    std::fs::create_dir_all(&dir).expect("create signed dir");
    dir
}

/// Writes the generated identity next to the artifacts.
///
/// This exists so the Node integration test can stand up a mock signing service
/// without generating a certificate chain of its own — Node cannot. The key is
/// thrown away with the build directory and never leaves it; a key committed to
/// the repository would be a different matter entirely.
pub fn write_test_identity(chain: &TestChain) {
    let dir = output_dir();
    std::fs::write(dir.join("test-key.pem"), &chain.key_pem).expect("write test key");
    std::fs::write(dir.join("test-chain.pem"), &chain.chain_pem).expect("write test chain");
}

/// Reads an artifact back and asserts that the parts our signer is responsible
/// for held up.
///
/// `validation_state` cannot be asserted directly: a private CA never satisfies
/// the trust check, so it is Invalid by construction. What must hold is that the
/// signature and the asset hash verified.
pub fn assert_signature_and_hash_valid(signed: &[u8], format: &str) {
    let reader = c2pa::Reader::from_context(c2pa::Context::new())
        .with_stream(format, Cursor::new(signed.to_vec()))
        .expect("read back the signed asset");
    let results = reader.validation_results().expect("validation results");
    let rendered = serde_json::to_string(results).expect("serialize validation results");

    assert!(
        rendered.contains("claimSignature.validated"),
        "signature did not validate: {rendered}"
    );
    assert!(
        !rendered.contains("claimSignature.mismatch"),
        "signature mismatch: {rendered}"
    );
    assert!(
        rendered.contains(if format == "image/avif" {
            "assertion.bmffHash.match"
        } else {
            "assertion.dataHash.match"
        }),
        "asset hash did not match: {rendered}"
    );
}
