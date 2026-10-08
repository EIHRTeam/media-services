import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

/** Optional independent verification, enabled in acceptance CI and local runs. */
export async function verifyArtifact(bytes, extension = "jpg") {
  if (!process.env.MPS_TEST_C2PATOOL) return;
  const temporary = await mkdtemp(join(tmpdir(), "mps-verify-"));
  try {
    const image = join(temporary, `signed.${extension}`);
    const settings = join(temporary, "settings.json");
    await writeFile(image, bytes);
    await writeFile(settings, JSON.stringify({ verify: { verify_trust: false } }));
    const { stdout } = await promisify(execFile)(
      process.env.MPS_TEST_C2PATOOL,
      [image, "-d", "--settings", settings],
      { maxBuffer: 16 * 1024 * 1024 },
    );
    const result = JSON.parse(stdout).validation_results.activeManifest;
    assert.deepEqual(result.failure ?? [], []);
    const successes = new Set(result.success.map((entry) => entry.code));
    assert.ok(successes.has("claimSignature.validated"));
    assert.ok(
      successes.has(extension === "avif" ? "assertion.bmffHash.match" : "assertion.dataHash.match"),
    );
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
