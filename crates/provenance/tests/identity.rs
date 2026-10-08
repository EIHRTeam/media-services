//! Generates the fixtures the Node suite needs.
//!
//! Kept as its own target so `pnpm gen:identity` can produce them without
//! running the whole Rust suite. It has to be re-runnable: cargo clears
//! `target/tmp` when it rebuilds the test binaries, so anything generated there
//! can disappear between two runs — which is exactly how the Node tests came to
//! fail with an ENOENT for a file they had no way to recreate.

mod support;

use support::{load_or_generate_chain, test_jpeg};

#[test]
fn write_test_identity_for_other_languages() {
    let chain = load_or_generate_chain();
    support::write_test_identity(&chain);

    // A plain input that has never been near the Rust side.
    let dir = support::output_dir();
    std::fs::write(dir.join("plain.jpg"), test_jpeg()).expect("write plain jpeg");

    std::fs::write(
        dir.join("plain.webp"),
        support::test_image(image::ImageFormat::WebP),
    )
    .expect("write plain webp");

    if let Some(root) = &chain.root_pem {
        std::fs::write(dir.join("root-ca.pem"), root).expect("write root ca");
    }

    eprintln!(
        "wrote test-key.pem, test-chain.pem and plain.jpg to {}",
        dir.display()
    );
}
