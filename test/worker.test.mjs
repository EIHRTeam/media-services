import { test, after } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { Worker } from "node:worker_threads";
import { setTimeout as delay } from "node:timers/promises";
import { createWorkerPool } from "../dist/node.js";
import { connect, readXmp } from "../dist/index.js";
import { verifyArtifact } from "./verify-artifact.mjs";
import { startMockSigner } from "./mock-signer.mjs";

const services = new Set();
async function mockSigner(options) {
  const service = await startMockSigner(options);
  services.add(service);
  return service;
}
after(async () => {
  await Promise.all([...services].map((service) => service.close()));
});

const TOKEN = "worker-test-token-never-shipped";
const image = () => readFile(new URL("../target/tmp/plain.jpg", import.meta.url));
const processOptions = {
  title: "image.jpg",
  manifest: {
    claim_version: 2,
    claim_generator_info: [{ name: "Worker Test" }],
    assertions: [
      {
        label: "c2pa.actions.v2",
        data: {
          actions: [
            {
              action: "c2pa.created",
              digitalSourceType: "http://cv.iptc.org/newscodes/digitalsourcetype/digitalCreation",
            },
          ],
        },
      },
    ],
  },
};
const until = async (predicate) => {
  for (let i = 0; i < 1000 && !predicate(); i++) await delay(2);
  assert.ok(predicate(), "condition timed out");
};

test("pool metadata roundtrip preserves input, transfers output and drains byte accounting", async () => {
  const pool = await createWorkerPool({ workers: 2 });
  try {
    const input = await image();
    const before = Uint8Array.from(input);
    const updated = await pool.writeXmp(input, { creatorTool: "pool test" });
    assert.deepEqual(Uint8Array.from(input), before);
    assert.match(await pool.readXmp(updated), /pool test/);
    assert.equal(pool.stats.inputBytes, 0);
    assert.equal(pool.stats.active, 0);
    assert.ok(pool.stats.wasmMemoryBytes > 0);
    const results = await Array.fromAsync(pool.writeXmpBatch([{ image: input }]));
    assert.equal(results[0].ok, true);
    assert.deepEqual(Uint8Array.from(input), before);
  } finally {
    await pool.close();
    await pool.close();
  }
  await assert.rejects(() => pool.readXmp(new Uint8Array()), { code: "invalidArgument" });
});

test("explicit transfer detaches input and rejects slices, shared buffers and pooled buffers", async () => {
  const pool = await createWorkerPool({ workers: 1 });
  try {
    const input = Uint8Array.from(await image());
    const result = pool.readXmp(input, { transfer: true });
    assert.equal(input.byteLength, 0);
    assert.equal(await result, undefined);
    for (const bytes of [
      new Uint8Array(16).subarray(1),
      new Uint8Array(new SharedArrayBuffer(16)),
      Buffer.from("tiny"),
    ]) {
      await assert.rejects(() => pool.readXmp(bytes, { transfer: true }), {
        code: "invalidArgument",
      });
      assert.ok(bytes.byteLength > 0);
    }
  } finally {
    await pool.close();
  }
});

test("worker sessions fetch identity once each and sign valid pipeline outputs", async () => {
  const service = await mockSigner({ token: TOKEN });
  const pool = await createWorkerPool({
    workers: 2,
    signing: { signerEndpoint: service.endpoint, token: TOKEN },
  });
  try {
    const results = await Array.fromAsync(
      pool.processBatch(
        Array.from({ length: 6 }, () => ({
          image: { byteLength: 0, load: async () => new Uint8Array() },
          options: processOptions,
        })),
      ),
    );
    assert.ok(results.every((result) => !result.ok));
    const input = await image();
    const signed = await Array.fromAsync(
      pool.processBatch(
        Array.from({ length: 4 }, () => ({ image: input, options: processOptions })),
      ),
    );
    assert.ok(signed.every((result) => result.ok));
    assert.ok(signed.every((result) => result.value.format === "image/jpeg"));
    await Promise.all(signed.map((result) => verifyArtifact(result.value.bytes)));
    assert.equal(service.requests.filter((r) => r.url === "/signer").length, 2);
    assert.equal(service.requests.filter((r) => r.url === "/sign").length, 4);
    assert.match(await readXmp(signed[0].value.bytes), /image\/jpeg/);
  } finally {
    await pool.close();
    await service.close();
  }
});

test("pool queue is bounded; queued cancellation releases its byte reservation", async () => {
  const service = await mockSigner({ token: TOKEN, delayMs: 60 });
  const pool = await createWorkerPool({
    workers: 1,
    maxQueuedTasks: 1,
    signing: { signerEndpoint: service.endpoint, token: TOKEN },
  });
  try {
    const input = await image();
    const first = pool.process(input, processOptions);
    const controller = new AbortController();
    const second = pool.process(input, processOptions, { signal: controller.signal });
    const rejection = assert.rejects(() => second, { code: "aborted" });
    await assert.rejects(() => pool.process(input, processOptions), { code: "queueFull" });
    controller.abort();
    await rejection;
    await first;
    assert.equal(pool.stats.inputBytes, 0);
  } finally {
    await pool.close();
    await service.close();
  }
});

test("request deadline and active cancellation keep error codes through C2PA", async () => {
  const service = await mockSigner({ token: TOKEN, hang: true });
  const pool = await createWorkerPool({
    workers: 1,
    signing: { signerEndpoint: service.endpoint, token: TOKEN },
  });
  try {
    const input = await image();
    await assert.rejects(() => pool.process(input, { ...processOptions, requestTimeoutMs: 20 }), {
      code: "timeout",
    });
    const controller = new AbortController();
    const before = service.requests.length;
    const pending = pool.process(input, processOptions, { signal: controller.signal });
    const rejection = assert.rejects(() => pending, { code: "aborted" });
    await until(() => service.requests.length > before);
    controller.abort();
    await rejection;
    assert.equal(pool.stats.inputBytes, 0);
  } finally {
    await pool.close();
    await service.close();
  }
});

test("disposing a session with pending work defers native free", async () => {
  const service = await mockSigner({ token: TOKEN, delayMs: 30 });
  const session = await connect({ signerEndpoint: service.endpoint, token: TOKEN });
  try {
    const pending = session.process(await image(), processOptions);
    session[Symbol.dispose]();
    await assert.rejects(() => session.process(new Uint8Array(), processOptions), {
      code: "invalidArgument",
    });
    assert.equal((await pending).format, "image/jpeg");
    await session.close();
    await session.close();
  } finally {
    await session.close();
    await service.close();
  }
});

test("worker crash fails current task and replacement accepts later work", async () => {
  const pool = await createWorkerPool({ workers: 1 });
  const original = Worker.prototype.postMessage;
  let killed = false;
  try {
    Worker.prototype.postMessage = function (message, ...args) {
      const result = original.call(this, message, ...args);
      if (message.type === "task" && !killed) {
        killed = true;
        void this.terminate();
      }
      return result;
    };
    await assert.rejects(() => pool.readXmp(new Uint8Array()), { code: "workerError" });
    Worker.prototype.postMessage = original;
    assert.equal(await pool.readXmp(await image()), undefined);
  } finally {
    Worker.prototype.postMessage = original;
    await pool.close();
  }
});

test("initialisation failure tears down all workers", async () => {
  const emptyModule = await WebAssembly.compile(new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]));
  await assert.rejects(() => createWorkerPool({ workers: 2, wasm: emptyModule }));
});

test("dispatch isolates clone failures without stranding the next queued task", async () => {
  const service = await mockSigner({ token: TOKEN, delayMs: 20 });
  const pool = await createWorkerPool({
    workers: 1,
    signing: { signerEndpoint: service.endpoint, token: TOKEN },
  });
  try {
    const input = await image();
    const first = pool.process(input, processOptions);
    const invalid = pool.process(input, {
      ...processOptions,
      manifest: { ...processOptions.manifest, invalid: () => {} },
    });
    const failure = assert.rejects(() => invalid);
    const last = pool.readXmp(input);
    await first;
    await failure;
    assert.equal(await last, undefined);
    assert.equal(pool.stats.inputBytes, 0);
  } finally {
    await pool.close();
    await service.close();
  }
});

test("pool close rejects queued tasks and terminates hung active tasks at its deadline", async () => {
  const service = await mockSigner({ token: TOKEN, hang: true });
  const pool = await createWorkerPool({
    workers: 1,
    closeTimeoutMs: 20,
    signing: { signerEndpoint: service.endpoint, token: TOKEN },
  });
  try {
    const input = await image();
    const first = pool.process(input, processOptions);
    const firstRejection = assert.rejects(() => first, { code: "aborted" });
    const queued = pool.readXmp(input);
    const queuedRejection = assert.rejects(() => queued, { code: "aborted" });
    await until(() => service.requests.some((request) => request.url === "/sign"));
    const start = Date.now();
    await pool.close();
    assert.ok(Date.now() - start < 1000);
    await Promise.all([firstRejection, queuedRejection]);
    assert.equal(pool.stats.workers, 0);
    assert.equal(pool.stats.inputBytes, 0);
  } finally {
    await pool.close();
    await service.close();
  }
});

test("cancelling one shared-session call does not cancel another", async () => {
  const service = await mockSigner({ token: TOKEN, delayMs: 30 });
  const session = await connect({ signerEndpoint: service.endpoint, token: TOKEN });
  try {
    const input = await image();
    const controller = new AbortController();
    const cancelled = session.process(input, { ...processOptions, signal: controller.signal });
    const failure = assert.rejects(() => cancelled, { code: "aborted" });
    const other = session.process(input, processOptions);
    await until(() => service.requests.filter((request) => request.url === "/sign").length === 2);
    controller.abort();
    await failure;
    assert.equal((await other).format, "image/jpeg");
  } finally {
    await session.close();
    await service.close();
  }
});

test("signer-info deadline includes a stalled response body", async () => {
  const { createServer } = await import("node:http");
  const server = createServer((_request, response) => {
    response.writeHead(200, { "content-type": "application/json" });
    response.flushHeaders();
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    await assert.rejects(
      () =>
        connect({
          signerEndpoint: `http://127.0.0.1:${server.address().port}`,
          token: TOKEN,
          requestTimeoutMs: 20,
        }),
      { code: "timeout" },
    );
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});

test("workers sign WebP and AVIF, with optional independent verification", async () => {
  const service = await mockSigner({ token: TOKEN });
  const pool = await createWorkerPool({
    workers: 2,
    signing: { signerEndpoint: service.endpoint, token: TOKEN },
  });
  try {
    for (const [extension, path] of [
      ["webp", new URL("../target/tmp/plain.webp", import.meta.url)],
      ["avif", new URL("../crates/xmp-image/tests/base.avif", import.meta.url)],
    ]) {
      const result = await pool.process(await readFile(path), {
        ...processOptions,
        title: `image.${extension}`,
      });
      assert.equal(result.format, `image/${extension}`);
      await verifyArtifact(result.bytes, extension);
    }
  } finally {
    await pool.close();
    await service.close();
  }
});

test("closing a pool cancels batch producers instead of reading the remaining collection", async () => {
  const service = await mockSigner({ token: TOKEN, hang: true });
  const pool = await createWorkerPool({
    workers: 1,
    signing: { signerEndpoint: service.endpoint, token: TOKEN },
  });
  try {
    const input = await image();
    let pulled = 0;
    let closed = false;
    function* source() {
      try {
        for (let i = 0; i < 1000; i++) {
          pulled++;
          yield { image: input, options: processOptions };
        }
      } finally {
        closed = true;
      }
    }
    const pending = Array.fromAsync(pool.processBatch(source()));
    const failure = assert.rejects(() => pending, { code: "aborted" });
    await until(() => service.requests.some((request) => request.url === "/sign"));
    await pool.close();
    await failure;
    assert.ok(pulled <= 2);
    assert.equal(closed, true);
  } finally {
    await pool.close();
    await service.close();
  }
});
