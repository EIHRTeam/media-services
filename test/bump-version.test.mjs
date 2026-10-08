/**
 * The version bump, and the drift it exists to prevent.
 *
 * A version lives in two hand-edited files and is copied into a third by cargo.
 * Updating the first two and committing without running cargo leaves Cargo.lock
 * stale, and every build that passes `--locked` — the wasm job and the release
 * workflow — then fails somewhere far from the cause. These tests pin both
 * halves: that the stale state really is refused, and that the script cannot
 * produce it.
 *
 * The fixture is a workspace with no dependencies, so it resolves offline and
 * the tests stay a check of this script rather than of the network.
 */
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { bumpVersion } from "../scripts/bump-version.mjs";

let root;

/** Whether a build that trusts the lockfile would accept this tree. */
function acceptsLockedBuild() {
  try {
    execFileSync("cargo", ["metadata", "--locked", "--format-version", "1"], {
      cwd: root,
      stdio: "ignore",
    });
    return true;
  } catch {
    return false;
  }
}

function lockedVersion(name) {
  const lock = read("Cargo.lock");
  const match = lock.match(
    new RegExp(`\\[\\[package\\]\\]\\nname = "${name}"\\nversion = "([^"]+)"`),
  );
  assert.ok(match, `Cargo.lock has no entry for ${name}`);
  return match[1];
}

function read(file) {
  // Synchronous, so a test can compare whole files with assert.deepEqual.
  return readFileSync(join(root, file), "utf8");
}

function cargo(args) {
  execFileSync("cargo", args, { cwd: root, stdio: "ignore" });
}

before(async () => {
  root = await mkdtemp(join(tmpdir(), "mps-bump-"));

  await writeFile(
    join(root, "Cargo.toml"),
    `[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "1.0.0"
edition = "2024"
`,
  );
  await writeFile(join(root, "package.json"), `{\n  "name": "fixture",\n  "version": "1.0.0"\n}\n`);

  // Two members: one inherits the workspace version, one carries its own. The
  // second is what tells a targeted update apart from one that rewrites every
  // entry it can reach.
  await mkdir(join(root, "crates/one/src"), { recursive: true });
  await writeFile(
    join(root, "crates/one/Cargo.toml"),
    `[package]\nname = "one"\nversion.workspace = true\nedition.workspace = true\n`,
  );
  await writeFile(join(root, "crates/one/src/lib.rs"), "");

  await mkdir(join(root, "crates/two/src"), { recursive: true });
  await writeFile(
    join(root, "crates/two/Cargo.toml"),
    `[package]\nname = "two"\nversion = "0.1.0"\nedition.workspace = true\n`,
  );
  await writeFile(join(root, "crates/two/src/lib.rs"), "");

  cargo(["generate-lockfile", "--offline"]);
});

after(async () => {
  await rm(root, { recursive: true, force: true });
});

test("the fixture starts in step", () => {
  assert.equal(lockedVersion("one"), "1.0.0");
  assert.ok(acceptsLockedBuild());
});

test("a manifest bumped on its own leaves the lock stale, and a locked build refuses it", () => {
  // The mistake that reached CI: Cargo.toml moved, nothing ran cargo, the commit
  // went out. Without this the regression test below would pass for the wrong
  // reason — a lockfile that is never checked at all.
  const original = read("Cargo.toml");
  writeFileSyncInRoot("Cargo.toml", original.replace(`version = "1.0.0"`, `version = "1.0.1"`));
  assert.equal(acceptsLockedBuild(), false);
  writeFileSyncInRoot("Cargo.toml", original);
  assert.ok(acceptsLockedBuild());
});

test("bumping through the script keeps all three files in step", () => {
  const result = bumpVersion("2.0.0", root);

  assert.equal(result.version, "2.0.0");
  assert.match(read("Cargo.toml"), /\[workspace\.package\]\nversion = "2\.0\.0"/);
  assert.equal(JSON.parse(read("package.json")).version, "2.0.0");
  assert.equal(lockedVersion("one"), "2.0.0");
  assert.ok(acceptsLockedBuild());
});

test("a member that declares its own version is left alone", () => {
  // `cargo update --workspace` reaches every workspace package; only the ones
  // inheriting the workspace version should move.
  assert.equal(lockedVersion("two"), "0.1.0");
});

test("bumping to the version already set writes nothing", () => {
  const files = ["Cargo.toml", "package.json", "Cargo.lock"];
  const before = files.map(read);

  const result = bumpVersion("2.0.0", root);

  assert.deepEqual(result.changed, []);
  assert.deepEqual(files.map(read), before);
});

test("a leading v is accepted, as it is on the release workflow's input", () => {
  const result = bumpVersion("v2.1.0-beta.1", root);

  assert.equal(result.version, "2.1.0-beta.1");
  assert.equal(JSON.parse(read("package.json")).version, "2.1.0-beta.1");
  assert.equal(lockedVersion("one"), "2.1.0-beta.1");
  assert.ok(acceptsLockedBuild());
});

test("a version the release workflow would reject is refused before anything is written", () => {
  const files = ["Cargo.toml", "package.json", "Cargo.lock"];
  const before = files.map(read);

  for (const version of ["2.0", "2.0.0.0", "", "v", "02.0.0", "2.0.0-", 2, null]) {
    assert.throws(
      () => bumpVersion(version, root),
      `expected ${JSON.stringify(version)} to be refused`,
    );
  }

  assert.deepEqual(files.map(read), before);
});

function writeFileSyncInRoot(file, contents) {
  // This stands in for a hand edit made before the commit, with nothing else
  // running to notice it.
  writeFileSync(join(root, file), contents);
}
