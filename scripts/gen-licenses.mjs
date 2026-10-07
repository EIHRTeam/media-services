#!/usr/bin/env node
// Collect upstream license texts for the locked WASM dependency tree.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const cargo = (args) => execFileSync("cargo", args, {
  cwd: root, encoding: "utf8", maxBuffer: 16 * 1024 * 1024,
});
const metadata = JSON.parse(cargo([
  "metadata", "--locked", "--format-version", "1", "--filter-platform", "wasm32-unknown-unknown",
]));
const tree = cargo([
  "tree", "--locked", "-p", "media-services-wasm", "--target", "wasm32-unknown-unknown",
  "--edges", "normal", "--prefix", "none", "--format", "{p}",
]);
const selected = new Set([...tree.matchAll(/^(\S+) v(\S+)/gm)].map((m) => `${m[1]}@${m[2]}`));
const packages = metadata.packages
  .filter((p) => p.source && selected.has(`${p.name}@${p.version}`))
  .sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`, "en"));

function licenseFiles(pkg) {
  const dir = dirname(pkg.manifest_path);
  const files = readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isFile() && /^(?:licen[cs]e|copying|notice|unlicense)(?:[._-]|$)/i.test(entry.name))
    .map((entry) => entry.name);
  if (pkg.license_file && !files.includes(pkg.license_file)) files.push(pkg.license_file);
  return files.sort().map((file) => `${file}\n\n${readFileSync(join(dir, file), "utf8")}`);
}

// Use the standard appendix placeholder, not this project's copyright, for upstream crates.
const apache = readFileSync(join(root, "LICENSE"), "utf8")
  .replace(/^(\s*)Copyright [^\r\n]+$/m, "$1Copyright [yyyy] [name of copyright owner]");
const sections = [
  "Third-party licenses for @eihrteam/mps-worker\n\n" +
  "Generated from Cargo.lock for media-services-wasm. Includes normal dependencies\n" +
  "and their procedural-macro dependencies; development-only dependencies are excluded.\n" +
  "Upstream license texts and notices follow. For dual-licensed packages without\n" +
  "packaged license files, distribution uses their Apache-2.0 option.\n",
];

for (const pkg of packages) {
  let texts = licenseFiles(pkg);
  if (texts.length === 0) {
    // Workspace subcrates sometimes omit their project's shared license files.
    const sibling = pkg.repository && packages.find((p) => p.repository === pkg.repository && p.license === pkg.license && licenseFiles(p).length > 0);
    if (sibling) texts = licenseFiles(sibling);
  }
  if (texts.length === 0 && pkg.name === "alloc-stdlib") {
    // Same project and BSD notice, verified at alloc-stdlib 0.2.4's VCS revision:
    // dropbox/rust-alloc-no-stdlib@ae42d22078b98549e987d2f03d12df7b984fde47/LICENSE
    const shared = packages.find((p) => p.name === "alloc-no-stdlib");
    if (shared) texts = licenseFiles(shared);
  }
  if (texts.length === 0 && /Apache-2\.0/.test(pkg.license ?? "")) {
    const source = join(dirname(pkg.manifest_path), "src/lib.rs");
    const notices = existsSync(source)
      ? (readFileSync(source, "utf8").slice(0, 2048).match(/^\/\/ Copyright[^\n]*/gm) ?? []).join("\n")
      : "";
    texts = [`Distributed under the Apache-2.0 option.\n${notices}\n\n${apache}`];
  }
  if (texts.length === 0) throw new Error(`No license text for ${pkg.name}@${pkg.version}`);
  sections.push([
    `${pkg.name} ${pkg.version}`,
    `Declared license: ${pkg.license}`,
    `Repository: ${pkg.repository ?? "https://crates.io/crates/" + pkg.name}`,
    ...pkg.authors.map((author) => `Author: ${author}`),
    "", ...texts,
  ].join("\n"));
}

mkdirSync(join(root, "pkg"), { recursive: true });
writeFileSync(join(root, "pkg/THIRD_PARTY_LICENSES.txt"), sections.join("\n\n" + "=".repeat(72) + "\n\n"));
console.log(`wrote pkg/THIRD_PARTY_LICENSES.txt (${packages.length} dependencies)`);
