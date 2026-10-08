#!/usr/bin/env node
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
const exec = promisify(execFile);
const root = dirname(dirname(fileURLToPath(import.meta.url)));
const temporary = await mkdtemp(join(tmpdir(), "mps-pack-"));
try {
  const packed = await exec(
    "npm",
    ["pack", "--json", "--ignore-scripts", "--pack-destination", temporary],
    { cwd: root },
  );
  const archive = join(temporary, JSON.parse(packed.stdout)[0].filename);
  await writeFile(
    join(temporary, "package.json"),
    JSON.stringify({ private: true, type: "module" }),
  );
  await exec("npm", ["install", "--ignore-scripts", "--no-audit", "--no-fund", archive], {
    cwd: temporary,
  });
  for (const args of [
    ["exec", "--offline", "--", "mps", "--version"],
    ["exec", "--offline", "@eihrteam/mps-worker", "--help"],
    ["exec", "--offline", "--package=@eihrteam/mps-worker", "--", "mps", "--help"],
  ]) {
    await exec("npm", args, { cwd: temporary });
  }
  await exec(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      `
    import assert from 'node:assert/strict';
    import { readXmp, writeXmp, readXmpBatch } from '@eihrteam/mps-worker';
    import { createWorkerPool } from '@eihrteam/mps-worker/node';
    // A minimal PNG with a valid IHDR and IDAT, copied by the smoke test only.
    const input = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==', 'base64');
    const output = await writeXmp(input, { creatorTool: 'packed test' });
    assert.match(await readXmp(output), /packed test/);
    assert.equal((await Array.fromAsync(readXmpBatch([{ image: output }])))[0].ok, true);
    await using pool = await createWorkerPool({ workers: 1 });
    assert.match(await pool.readXmp(output), /packed test/);
  `,
    ],
    { cwd: temporary },
  );
  await writeFile(
    join(temporary, "consumer.mts"),
    `
    import { readXmpBatch, type BatchSource, type XmpEdit, type ProcessOptions } from '@eihrteam/mps-worker';
    import { createWorkerPool, type WorkerPoolOptions } from '@eihrteam/mps-worker/node';
    const source: BatchSource = [{ image: new Uint8Array() }];
    const edits: XmpEdit = { creatorTool: 'type smoke' };
    const signing: ProcessOptions = { title: 'image.jpg', manifest: {} };
    const options: WorkerPoolOptions = { workers: 1 };
    async function consume() {
      await using pool = await createWorkerPool(options);
      await pool.writeXmp(new Uint8Array(), edits);
      for await (const result of readXmpBatch(source)) if (!result.ok) console.log(result.error.code);
      for await (const result of pool.processBatch([{ image: new Uint8Array(), options: signing }])) {
        if (result.ok) console.log(result.value.bytes.byteLength);
      }
    }
    void consume;
  `,
  );
  await exec(
    process.execPath,
    [
      join(root, "node_modules/typescript/lib/tsc.js"),
      "--noEmit",
      "--strict",
      "--module",
      "NodeNext",
      "--target",
      "ES2024",
      "--lib",
      "ES2024,DOM,ESNext.Disposable",
      "--skipLibCheck",
      "--typeRoots",
      join(root, "node_modules/@types"),
      "consumer.mts",
    ],
    { cwd: temporary },
  );
  console.log(
    "test:pack passed — installed tarball, bin inference, explicit mps, WASM/Worker loading and consumer declarations",
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
}
