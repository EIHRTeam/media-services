#!/usr/bin/env node
/**
 * Checks that nothing secret can reach a published package.
 *
 * Signing is a capability: a packaged private key, or a live bearer token, is a
 * far worse outcome than a bug. The file list comes from `npm pack --dry-run`
 * rather than from `files` in package.json, because the point is to inspect what
 * would actually be published.
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

/** Patterns that must never appear in a shipped file. */
const FORBIDDEN = [
  { name: "PEM private key", pattern: /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/ },
  { name: "PKCS#8 private key", pattern: /-----BEGIN PRIVATE KEY-----/ },
  { name: "bearer token literal", pattern: /\bBearer\s+[A-Za-z0-9._~+/-]{20,}=*/ },
];

function scan(label, contents) {
  const text = contents.toString("latin1");
  return FORBIDDEN.filter(({ pattern }) => pattern.test(text)).map(
    ({ name }) => `${label}: ${name}`,
  );
}

// Self-test first. A scanner that silently matches nothing would report success
// on every input, which is the one failure mode that matters here.
const planted = scan("self-test", "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----");
if (planted.length === 0) {
  console.error("check:bundle is broken — it did not detect a planted private key");
  process.exit(1);
}

const packed = JSON.parse(
  execFileSync("npm", ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: root,
    encoding: "utf8",
    // npm writes notices to stderr; stdout stays parseable JSON.
    stdio: ["ignore", "pipe", "ignore"],
  }),
);

const files = packed[0]?.files ?? [];
if (files.length === 0) {
  console.error("check:bundle found no files to inspect — is the package built?");
  process.exit(1);
}

const findings = [];
const paths = new Set(files.map((file) => file.path));
const required = [
  "package.json",
  "README.md",
  "LICENSE",
  "dist/index.js",
  "dist/index.d.ts",
  "dist/errors.js",
  "dist/loader.js",
  "dist/result.js",
  "js/index.ts",
  "js/errors.ts",
  "js/loader.ts",
  "js/result.ts",
  "pkg/media_services_wasm.js",
  "pkg/media_services_wasm.d.ts",
  "pkg/media_services_wasm_bg.wasm",
  "pkg/THIRD_PARTY_LICENSES.txt",
];
for (const path of required) {
  if (!paths.has(path)) findings.push(`missing required package file: ${path}`);
}
for (const file of files) {
  if (
    !/^(?:package\.json|README\.md|LICENSE|js\/(?:index|errors|loader|result)\.ts|dist\/(?:index|errors|loader|result)\.(?:js|d\.ts)(?:\.map)?|pkg\/THIRD_PARTY_LICENSES\.txt|pkg\/media_services_wasm(?:_bg)?\.(?:js|d\.ts|wasm|wasm\.d\.ts))$/.test(
      file.path,
    )
  ) {
    findings.push(`unexpected package file: ${file.path}`);
  }
  // Generated bindings and the wasm binary are large and binary; the wasm is
  // still scanned because a key baked into it would be the worst case.
  let contents;
  try {
    contents = readFileSync(join(root, file.path));
  } catch (error) {
    findings.push(`${file.path}: cannot inspect packaged file (${error.code ?? "read error"})`);
    continue;
  }
  if (
    file.path.endsWith(".wasm") &&
    !contents.subarray(0, 8).equals(Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]))
  ) {
    findings.push(`${file.path}: invalid WebAssembly header`);
  }
  findings.push(...scan(file.path, contents));
}

if (findings.length > 0) {
  console.error("package payload check failed:");
  for (const finding of findings) console.error(`  ${finding}`);
  process.exit(1);
}

console.log(
  `check:bundle passed — ${files.length} files inspected, required artifacts present, no matched secret patterns`,
);
