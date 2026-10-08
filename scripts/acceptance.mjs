#!/usr/bin/env node
/**
 * Independent acceptance: check what the pipeline produced with `c2patool`.
 *
 * The Rust tests assert against the same c2pa version that produced the
 * artifacts, which is a self-consistency check. This is the outside opinion — a
 * different program, built from a pinned release, reading the files cold.
 *
 * Run `cargo test -p provenance` first; the artifacts land in target/tmp/signed.
 *
 * Use c2patool 0.28.1. Every artifact must pass signature, certificate-profile,
 * and asset-hash validation. Public trust is not checked: the test suite mints
 * its own private CA. An explicit settings file avoids ambient user settings.
 */
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
// Signed artifacts only: inputs and fixtures sit alongside them, and c2patool
// exits non-zero for a file that was never meant to carry a manifest.
const ARTIFACTS = join(root, "target/tmp/signed");
const SETTINGS = join(root, "target/tmp/acceptance-settings.json");
const XMP_HEADER = Buffer.from("http://ns.adobe.com/xap/1.0/\0", "ascii");
const PNG_XMP_KEYWORD = Buffer.from("XML:com.adobe.xmp", "ascii");
const WEBP_XMP_CHUNK = Buffer.from("XMP ", "ascii");

/**
 * What each artifact should look like. `signed.jpg` comes from the signing test,
 * which signs a bare image to exercise the protocol — so it carries no metadata
 * by design, and demanding XMP of it would be testing the wrong thing.
 */
const EXPECTATIONS = {
  "signed.jpg": { xmp: false },
  "stamped.jpg": { xmp: true },
  "stamped.webp": { xmp: true },
  "stamped.avif": { xmp: true },
};

function c2patool(path) {
  const output = execFileSync("c2patool", [path, "-d", "--settings", SETTINGS], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  return JSON.parse(output);
}

function carriesXmp(bytes) {
  return (
    bytes.includes(XMP_HEADER) ||
    bytes.includes(PNG_XMP_KEYWORD) ||
    bytes.includes(WEBP_XMP_CHUNK) ||
    bytes.includes(Buffer.from("application/rdf+xml"))
  );
}

if (!existsSync(ARTIFACTS)) {
  console.error(`no signed artifacts in ${ARTIFACTS}; run \`cargo test -p provenance\` first`);
  process.exit(1);
}

const artifacts = readdirSync(ARTIFACTS).filter((name) => /\.(jpe?g|png|webp|avif)$/i.test(name));
if (artifacts.length === 0) {
  console.error(`no signed artifacts in ${ARTIFACTS}; run \`cargo test -p provenance\` first`);
  process.exit(1);
}

let problems = 0;
writeFileSync(SETTINGS, JSON.stringify({ verify: { verify_trust: false } }));
console.log("note: public certificate trust is not checked for the private test CA");

for (const name of artifacts) {
  const path = join(ARTIFACTS, name);
  const bytes = readFileSync(path);
  const report = c2patool(path);
  const expectations = EXPECTATIONS[name] ?? { xmp: false };

  const successes = new Set(
    (report.validation_results?.activeManifest?.success ?? []).map((entry) => entry.code),
  );
  const failures = (report.validation_results?.activeManifest?.failure ?? []).map(
    (entry) => entry.code,
  );

  const violations = [];

  if (
    !successes.has(name.endsWith(".avif") ? "assertion.bmffHash.match" : "assertion.dataHash.match")
  ) {
    violations.push("asset hash did not match");
  }
  if (!successes.has("claimSignature.validated")) {
    violations.push("signature did not validate");
  }
  for (const failure of failures) {
    violations.push(`validation failure: ${failure}`);
  }
  if (expectations.xmp && !carriesXmp(bytes)) {
    violations.push("the XMP written before signing is missing from the file");
  }

  if (violations.length > 0) {
    problems += 1;
    console.error(`FAIL ${name}: ${violations.join("; ")}`);
  } else {
    console.log(`ok   ${name}: signature valid, asset hash matched`);
  }
}

if (problems > 0) {
  console.error(`${problems} of ${artifacts.length} artifacts failed`);
  process.exit(1);
}
console.log(`all ${artifacts.length} artifacts passed signature and asset-hash validation`);
