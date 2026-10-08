#!/usr/bin/env node
/**
 * Moves the version in Cargo.toml, package.json and Cargo.lock together.
 *
 * The version is written by hand in two manifest files and copied into a third
 * by cargo, and cargo only rewrites the lockfile when it is asked to. Bumping
 * the two manifests and committing is the natural thing to do, and it produces a
 * tree whose every `--locked` build — the wasm job and the release workflow —
 * fails somewhere well away from the actual mistake. So the three moves are
 * made one operation here rather than three remembered ones.
 *
 * `cargo update --workspace` is the narrow tool for it: it reaches exactly the
 * workspace packages, where the version lives, and cannot move a third-party
 * dependency that a plain `cargo update` would float.
 *
 *   pnpm version:bump 1.1.0
 */
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

/**
 * The same pattern publish.yml validates the requested version against, kept
 * deliberately identical: a version this accepts but the release workflow
 * rejects would be found at release time, with the tag already pushed and the
 * commit already on main.
 */
const SEMVER =
  /^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*)?$/;

const SECTION = "[workspace.package]";

/**
 * Replaces the version inside `[workspace.package]` and nowhere else.
 *
 * A match over the whole file would also catch a `version` key in any crate's
 * manifest that happens to declare its own, which is a real shape here: the
 * members inherit, but nothing stops one of them from pinning instead.
 */
export function replaceWorkspaceVersion(source, version) {
  const start = source.indexOf(SECTION);
  if (start === -1) throw new Error("Cargo.toml has no [workspace.package] section");
  const bodyStart = start + SECTION.length;
  const sectionEnd = source.slice(bodyStart).search(/\n\[/);
  const body =
    sectionEnd === -1 ? source.slice(bodyStart) : source.slice(bodyStart, bodyStart + sectionEnd);
  const pattern = /^(\s*version\s*=\s*)"[^"]*"/m;
  // Tested for a match rather than for a change: bumping to the version already
  // set replaces the text with itself, and that is not a failure.
  if (!pattern.test(body)) throw new Error("Cargo.toml has no version in [workspace.package]");
  const replaced = body.replace(pattern, `$1"${version}"`);
  const rest = sectionEnd === -1 ? "" : source.slice(bodyStart + sectionEnd);
  return source.slice(0, bodyStart) + replaced + rest;
}

function normaliseVersion(input) {
  // A leading v is stripped because the release workflow's input accepts one,
  // and the two entry points should not disagree about what a version looks
  // like.
  const version = typeof input === "string" ? input.replace(/^v/, "") : "";
  if (!SEMVER.test(version)) {
    throw new Error(
      `not a version: ${JSON.stringify(input)}. Use semver such as 1.1.0 or 1.1.0-beta.1.`,
    );
  }
  return version;
}

/**
 * Writes the version into both manifests and refreshes the lockfile.
 *
 * Validates first and writes last, so a refused version leaves the tree exactly
 * as it was rather than half-bumped.
 */
export function bumpVersion(input, base = root) {
  const version = normaliseVersion(input);

  const cargoToml = join(base, "Cargo.toml");
  const packageJson = join(base, "package.json");
  const lock = join(base, "Cargo.lock");

  const tomlBefore = readFileSync(cargoToml, "utf8");
  const jsonBefore = readFileSync(packageJson, "utf8");

  const manifest = JSON.parse(jsonBefore);
  manifest.version = version;

  const tomlAfter = replaceWorkspaceVersion(tomlBefore, version);
  // Serialised by hand rather than substituted as text: package.json is a
  // parsed document, and a nested `version` key would be a silent casualty of a
  // textual replace.
  const jsonAfter = `${JSON.stringify(manifest, null, 2)}\n`;

  // The manifests go first: `cargo update` reads them to learn what the
  // workspace version now is.
  const changed = [];
  for (const [file, before, after] of [
    [cargoToml, tomlBefore, tomlAfter],
    [packageJson, jsonBefore, jsonAfter],
  ]) {
    if (before === after) continue;
    writeFileSync(file, after);
    changed.push(file);
  }

  const lockBefore = readFileSync(lock, "utf8");
  execFileSync("cargo", ["update", "--workspace"], { cwd: base, stdio: "pipe" });
  if (readFileSync(lock, "utf8") !== lockBefore) changed.push(lock);

  return { version, changed: changed.map((file) => relative(base, file)) };
}

function relative(base, file) {
  return file.slice(base.endsWith("/") ? base.length : base.length + 1);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const input = process.argv[2];
  if (!input) {
    console.error("usage: pnpm version:bump <version>");
    process.exit(1);
  }
  let result;
  try {
    result = bumpVersion(input);
  } catch (error) {
    // `cargo update` reports through stderr, which is captured rather than
    // inherited so the function stays quiet when a test calls it.
    console.error(String(error.stderr ?? error.message).trim());
    process.exit(1);
  }
  if (result.changed.length === 0) {
    console.log(`already at ${result.version}; nothing to do`);
  } else {
    console.log(`version is now ${result.version}`);
    for (const file of result.changed) console.log(`  ${file}`);
    console.log("review the change, then commit the three files together");
  }
}
