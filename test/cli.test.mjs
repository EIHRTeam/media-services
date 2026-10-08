import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, mkdir, readdir, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import { readXmp } from "../dist/index.js";
import { verifyArtifact } from "./verify-artifact.mjs";
import { startMockSigner } from "./mock-signer.mjs";

const cli = fileURLToPath(new URL("../dist/cli.js", import.meta.url));
// Read rather than written out here. The assertion below is that the CLI
// reports the version of the package it ships in, and a literal in the test is
// a second copy of that version which no release updates — which is how this
// came to fail when the workspace moved to 1.1.0.
const { version } = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
let directory;
let input;
let edit;
before(async () => {
  directory = await mkdtemp(join(tmpdir(), "mps-cli-"));
  input = join(directory, "input.jpg");
  edit = join(directory, "edit.json");
  await writeFile(input, await readFile(new URL("../target/tmp/plain.jpg", import.meta.url)));
  await writeFile(edit, JSON.stringify({ creatorTool: "CLI regression" }));
});
after(async () => {
  await rm(directory, { recursive: true, force: true });
});
function invoke(args, env = {}) {
  const child = spawn(process.execPath, [cli, ...args], {
    env: { ...process.env, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8").on("data", (text) => {
    stdout += text;
  });
  child.stderr.setEncoding("utf8").on("data", (text) => {
    stderr += text;
  });
  const result = new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code, signal) => resolve({ code, signal, stdout, stderr }));
  });
  return { child, result };
}
const run = (args, env) => invoke(args, env).result;

test("CLI help, version and parameter errors", async () => {
  assert.match((await run(["--help"])).stdout, /mps xmp read/);
  assert.equal((await run(["--version"])).stdout.trim(), version);
  for (const args of [
    ["xmp", "write", input],
    ["xmp", "write", input, "--edit", edit, "--output-dir", join(directory, "invalid-single")],
    ["sign", input, "--output", join(directory, "missing.jpg")],
    ["--workers", "bogus", "xmp", "read", input],
  ]) {
    assert.equal((await run(args)).code, 2);
  }
});

test("CLI writes atomically, reads XML, refuses overwrite and supports force", async () => {
  const output = join(directory, "single.jpg");
  const args = ["xmp", "write", input, "--edit", edit, "--output", output];
  assert.equal((await run(args)).code, 0);
  assert.match((await run(["xmp", "read", output])).stdout, /CLI regression/);
  assert.equal((await run(args)).code, 1);
  assert.equal((await run([...args, "--force", "--workers", "1"])).code, 0);
  assert.ok(!(await readdir(directory)).some((name) => name.startsWith(".mps-")));
});

test("CLI recursive directory traversal skips symlinks, unsupported extensions and output tree", async () => {
  const root = join(directory, "tree");
  await mkdir(join(root, "nested"), { recursive: true });
  await writeFile(join(root, "a.jpg"), await readFile(input));
  await writeFile(join(root, "nested", "b.jpg"), await readFile(input));
  await writeFile(join(root, "ignore.txt"), "text");
  await symlink(input, join(root, "link.jpg"));
  const output = join(root, "out");
  await mkdir(output);
  await writeFile(join(output, "old.jpg"), await readFile(input));
  const response = await run([
    "xmp",
    "write",
    root,
    "--recursive",
    "--edit",
    edit,
    "--output-dir",
    output,
    "--workers",
    "2",
    "--json",
  ]);
  assert.equal(response.code, 0, response.stderr);
  const records = response.stdout.trim().split("\n").map(JSON.parse);
  assert.equal(records.length, 2);
  assert.ok(records.every((record) => record.ok));
  assert.match(await readXmp(await readFile(join(output, "nested", "b.jpg"))), /CLI regression/);
});

test("CLI batches isolate failures and produce JSONL", async () => {
  const bad = join(directory, "bad.jpg");
  await writeFile(bad, "not an image");
  const result = await run([
    "xmp",
    "read",
    input,
    bad,
    "--json",
    "--max-input-bytes",
    "1MiB",
    "--max-in-flight-bytes",
    "2MiB",
  ]);
  assert.equal(result.code, 1);
  const records = result.stdout.trim().split("\n").map(JSON.parse);
  assert.equal(records.length, 2);
  assert.equal(records.filter((record) => record.ok).length, 1);
  assert.equal(records.find((record) => !record.ok).error.code, "unsupportedFormat");
});

test("CLI sign uses caller manifest and environment credentials", async () => {
  const token = "cli-test-token-never-shipped";
  const service = await startMockSigner({ token });
  try {
    const manifest = join(directory, "manifest.json");
    await writeFile(
      manifest,
      JSON.stringify({
        claim_version: 2,
        claim_generator_info: [{ name: "CLI Test" }],
        assertions: [
          {
            label: "c2pa.actions.v2",
            data: {
              actions: [
                {
                  action: "c2pa.created",
                  digitalSourceType:
                    "http://cv.iptc.org/newscodes/digitalsourcetype/digitalCreation",
                },
              ],
            },
          },
        ],
      }),
    );
    const output = join(directory, "signed.jpg");
    const result = await run(
      [
        "sign",
        input,
        "--manifest",
        manifest,
        "--edit",
        edit,
        "--output",
        output,
        "--workers",
        "1",
        "--json",
      ],
      { SIGNER_ENDPOINT: service.endpoint, SIGNER_TOKEN: token },
    );
    assert.equal(result.code, 0, result.stderr);
    assert.equal(JSON.parse(result.stdout).ok, true);
    await verifyArtifact(await readFile(output));
    assert.match(await readXmp(await readFile(output)), /CLI regression/);
    assert.equal(service.requests.filter((request) => request.url === "/sign").length, 1);
  } finally {
    await service.close();
  }
});

test("CLI SIGINT cancels hanging signing and leaves no temporary output", async () => {
  const token = "cli-interrupt-token-never-shipped";
  const service = await startMockSigner({ token, hang: true });
  try {
    const manifest = join(directory, "interrupt-manifest.json");
    const output = join(directory, "interrupt.jpg");
    await writeFile(
      manifest,
      JSON.stringify({ claim_version: 2, claim_generator_info: [{ name: "CLI Interrupt" }] }),
    );
    const running = invoke(
      ["sign", input, "--manifest", manifest, "--output", output, "--workers", "1"],
      { SIGNER_ENDPOINT: service.endpoint, SIGNER_TOKEN: token },
    );
    for (let i = 0; i < 1000 && !service.requests.some((request) => request.url === "/sign"); i++)
      await delay(2);
    assert.ok(service.requests.some((request) => request.url === "/sign"));
    running.child.kill("SIGINT");
    assert.equal((await running.result).code, 130);
    const names = await readdir(directory);
    assert.ok(!names.includes("interrupt.jpg"));
    assert.ok(!names.some((name) => name.startsWith(".mps-")));
  } finally {
    await service.close();
  }
});
