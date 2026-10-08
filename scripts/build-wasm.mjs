#!/usr/bin/env node
/**
 * Builds the WebAssembly module and its bindings.
 *
 * Two things here are load-bearing rather than ceremony. `RUSTFLAGS` must not be
 * set: an exported RUSTFLAGS replaces the `[target.wasm32-unknown-unknown]`
 * table in `.cargo/config.toml` wholesale, which silently drops the
 * `getrandom_backend` cfg and produces a module that fails at runtime rather
 * than at build time. And the `wasm-bindgen` CLI must match the crate version in
 * the lockfile exactly, which is checked up front because the alternative is a
 * confusing error much later.
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const CRATE = "media-services-wasm";
const OUT_NAME = "media_services_wasm";
const OUT_DIR = join(root, "pkg");

/**
 * Ceiling on the shipped module, set as a dead line rather than a target. The
 * module is around 9 MB, so this leaves room for growth while still failing the
 * build on a large accidental addition — pulling in a dependency that should not
 * be there. A size optimisation that fails ships a larger module, and that is
 * reported rather than fatal.
 */
const SIZE_BUDGET_BYTES = 15 * 1024 * 1024;

function run(command, args, options = {}) {
  return execFileSync(command, args, { cwd: root, encoding: "utf8", ...options });
}

/**
 * Finds a tool, falling back to cargo's install directory.
 *
 * `cargo install` puts binaries in `~/.cargo/bin`, which rustup only adds to
 * interactive shells — so a build driven from a script or CI frequently cannot
 * see a tool that is in fact installed.
 */
function resolveBinary(name) {
  const cargoBin = join(process.env.CARGO_HOME ?? join(homedir(), ".cargo"), "bin", name);
  for (const candidate of [name, cargoBin]) {
    try {
      execFileSync(candidate, ["--version"], { stdio: "ignore" });
      return candidate;
    } catch {
      // Try the next candidate.
    }
  }
  return undefined;
}

function crateVersion() {
  const lock = readFileSync(join(root, "Cargo.lock"), "utf8");
  // Each lock entry begins with `[[package]]`, so the name line is not at the
  // start of the block.
  const match = lock.match(/\[\[package\]\]\nname = "wasm-bindgen"\nversion = "([^"]+)"/);
  if (!match?.[1]) throw new Error("wasm-bindgen is not in Cargo.lock");
  return match[1];
}

function assertToolchain() {
  const expected = crateVersion();
  const binary = resolveBinary("wasm-bindgen");
  if (!binary) {
    throw new Error(
      `wasm-bindgen CLI not found. Install the matching version with:\n` +
        `  cargo install wasm-bindgen-cli --version ${expected} --locked`,
    );
  }
  const actual = run(binary, ["--version"])
    .trim()
    .replace(/^wasm-bindgen\s+/, "");
  if (actual !== expected) {
    throw new Error(
      `wasm-bindgen CLI is ${actual} but Cargo.lock pins ${expected}.\n` +
        `  cargo install wasm-bindgen-cli --version ${expected} --locked --force`,
    );
  }

  // See the note at the top of this file.
  for (const variable of ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]) {
    if (process.env[variable]) {
      throw new Error(
        `${variable} is set, which would override .cargo/config.toml and drop the ` +
          `getrandom wasm backend cfg. Unset it:\n  unset ${variable}`,
      );
    }
  }
  return { version: expected, binary };
}

function megabytes(bytes) {
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

const { version: wasmBindgenVersion, binary: wasmBindgen } = assertToolchain();
console.log(`wasm-bindgen ${wasmBindgenVersion}`);

run(
  "cargo",
  ["build", "--locked", "--release", "--target", "wasm32-unknown-unknown", "-p", CRATE],
  {
    stdio: "inherit",
  },
);

const artifact = join(
  root,
  "target/wasm32-unknown-unknown/release",
  `${CRATE.replaceAll("-", "_")}.wasm`,
);
console.log(`compiled: ${megabytes(statSync(artifact).size)}`);

mkdirSync(OUT_DIR, { recursive: true });
run(wasmBindgen, ["--target", "web", "--out-dir", OUT_DIR, "--out-name", OUT_NAME, artifact]);

const binding = join(OUT_DIR, `${OUT_NAME}_bg.wasm`);
const before = statSync(binding).size;
const wasmOpt = resolveBinary("wasm-opt");
if (!wasmOpt) {
  // A size optimisation, not a correctness requirement.
  console.warn(`wasm-opt not found; shipping ${megabytes(before)} unoptimised`);
} else {
  try {
    // rustc emits sign-extension, bulk-memory and non-trapping float-to-int
    // instructions by default on wasm32, and wasm-opt refuses to read any of
    // them unless told they are allowed. All three are named because the
    // defaults differ by binaryen version: sign extension is on in newer
    // releases but off in the 108 that Ubuntu ships, and a build that only works
    // against one of them is a build that fails in CI. Chromium 130 and Node 24
    // both support the three.
    run(wasmOpt, [
      "-Os",
      "--enable-sign-ext",
      "--enable-bulk-memory",
      "--enable-nontrapping-float-to-int",
      "--strip-debug",
      "--strip-producers",
      binding,
      "-o",
      binding,
    ]);
    console.log(`wasm-opt: ${megabytes(before)} -> ${megabytes(statSync(binding).size)}`);
  } catch (error) {
    // Failing to shrink the module must not fail the build, but it should be
    // visible rather than silently reported as "not available".
    console.warn(`wasm-opt failed; shipping ${megabytes(before)} unoptimised`);
    console.warn(String(error.stderr ?? error.message).split("\n")[0]);
  }
}

const finalSize = statSync(binding).size;
if (finalSize > SIZE_BUDGET_BYTES) {
  console.error(
    `the module is ${megabytes(finalSize)}, over the ${megabytes(SIZE_BUDGET_BYTES)} budget`,
  );
  process.exit(1);
}
console.log(`module: ${megabytes(finalSize)} (budget ${megabytes(SIZE_BUDGET_BYTES)})`);
