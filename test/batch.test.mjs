import { test } from "node:test";
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { runBatch } from "../dist/batch.js";

const limits = {
  concurrency: 2,
  maxOutstanding: 3,
  maxInputBytes: 8,
  maxOutputBytes: 8,
  maxInFlightBytes: 16,
  maxBufferedBytes: 16,
};
const item = (id, length = 4) => ({ id, image: new Uint8Array(length) });
const collect = async (source) => Array.fromAsync(source);

test("batch completes out of order and preserves identity", async () => {
  const batch = runBatch(
    [
      { ...item("slow"), options: 30 },
      { ...item("fast"), options: 1 },
    ],
    async (_image, ms) => {
      await delay(ms);
      return new Uint8Array(1);
    },
    limits,
  );
  const results = await collect(batch);
  assert.deepEqual(
    results.map(({ id, index }) => [id, index]),
    [
      ["fast", 1],
      ["slow", 0],
    ],
  );
});

test("slow consumer bounds pulling, loading and outstanding results", async () => {
  let pulled = 0;
  let loaded = 0;
  let active = 0;
  let peak = 0;
  function* source() {
    for (let i = 0; i < 100; i++) {
      pulled++;
      yield {
        image: {
          byteLength: 8,
          load: async () => {
            loaded++;
            return new Uint8Array(8);
          },
        },
      };
    }
  }
  const batch = runBatch(
    source(),
    async () => {
      active++;
      peak = Math.max(peak, active);
      await delay(1);
      active--;
      return new Uint8Array(8);
    },
    limits,
  );
  await batch.next();
  await delay(20);
  assert.ok(pulled <= limits.maxOutstanding + 1);
  assert.ok(loaded <= limits.maxOutstanding);
  assert.ok(peak <= 2);
  let count = 1;
  for await (const result of batch) {
    assert.equal(result.ok, true);
    count++;
  }
  assert.equal(count, 100);
});

test("reserves input byte budget before invoking lazy loaders", async () => {
  let activeBytes = 0;
  let peak = 0;
  const source = Array.from({ length: 12 }, () => ({
    image: {
      byteLength: 8,
      load: async () => {
        activeBytes += 8;
        peak = Math.max(peak, activeBytes);
        await delay(1);
        return new Uint8Array(8);
      },
    },
  }));
  await collect(
    runBatch(
      source,
      async () => {
        await delay(2);
        activeBytes -= 8;
        return undefined;
      },
      { ...limits, concurrency: 4, maxInFlightBytes: 8 },
    ),
  );
  assert.equal(peak, 8);
});

test("input, loader, execution and output failures are isolated", async () => {
  const source = [
    item("large", 9),
    { id: "size", image: { byteLength: 4, load: async () => new Uint8Array(5) } },
    {
      id: "load",
      image: {
        byteLength: 4,
        load: async () => {
          throw new Error("disk failed");
        },
      },
    },
    { ...item("output"), options: "large" },
    item("good"),
  ];
  const results = await collect(
    runBatch(source, async (_image, kind) => new Uint8Array(kind === "large" ? 9 : 4), limits),
  );
  const byId = Object.fromEntries(results.map((result) => [result.id, result]));
  assert.equal(byId.large.error.code, "resourceLimit");
  assert.equal(byId.size.error.code, "invalidArgument");
  assert.match(byId.load.error.message, /disk failed/);
  assert.equal(byId.output.error.code, "resourceLimit");
  assert.equal(byId.good.ok, true);
});

test("invalid budgets fail immediately", async () => {
  await assert.rejects(
    () => collect(runBatch([], async () => undefined, { ...limits, maxBufferedBytes: 1 })),
    { code: "invalidArgument" },
  );
  await assert.rejects(
    () => collect(runBatch([], async () => undefined, { ...limits, concurrency: NaN })),
    { code: "invalidArgument" },
  );
});

test("source failure cancels active work and closes producer", async () => {
  let closed = false;
  let cancelled = false;
  async function* source() {
    try {
      yield item("pending");
      throw new Error("source failed");
    } finally {
      closed = true;
    }
  }
  const batch = runBatch(
    source(),
    async (_image, _options, signal) => {
      await new Promise((resolve) =>
        signal.addEventListener(
          "abort",
          () => {
            cancelled = true;
            resolve();
          },
          { once: true },
        ),
      );
    },
    limits,
  );
  await assert.rejects(() => collect(batch), /source failed/);
  assert.equal(closed, true);
  assert.equal(cancelled, true);
});

test("break cancels remaining tasks and returns source", async () => {
  let closed = false;
  let cancelled = false;
  function* source() {
    try {
      yield { ...item("first"), options: false };
      yield { ...item("second"), options: true };
    } finally {
      closed = true;
    }
  }
  for await (const result of runBatch(
    source(),
    async (_image, pending, signal) => {
      if (pending)
        await new Promise((resolve) =>
          signal.addEventListener(
            "abort",
            () => {
              cancelled = true;
              resolve();
            },
            { once: true },
          ),
        );
      else await delay(5);
      return undefined;
    },
    limits,
  )) {
    assert.equal(result.id, "first");
    break;
  }
  assert.equal(closed, true);
  assert.equal(cancelled, true);
});

test("external cancellation rejects the batch and reaches lazy loader", async () => {
  const controller = new AbortController();
  let cancelled = false;
  const source = [
    {
      image: {
        byteLength: 4,
        load: async (signal) => {
          await new Promise((resolve) =>
            signal.addEventListener(
              "abort",
              () => {
                cancelled = true;
                resolve();
              },
              { once: true },
            ),
          );
          return new Uint8Array(4);
        },
      },
    },
  ];
  setTimeout(() => controller.abort(), 10);
  await assert.rejects(
    () =>
      collect(runBatch(source, async () => undefined, { ...limits, signal: controller.signal })),
    { code: "aborted" },
  );
  assert.equal(cancelled, true);
});

test("malformed iterator results terminate instead of hanging; invalid items retain indices", async () => {
  await assert.rejects(
    () =>
      collect(
        runBatch(
          {
            [Symbol.iterator]() {
              return { next: () => null };
            },
          },
          async () => undefined,
          limits,
        ),
      ),
    { code: "invalidArgument" },
  );
  const results = await collect(
    runBatch([undefined, null, item("good")], async () => undefined, limits),
  );
  assert.deepEqual(
    results.map((result) => [result.index, result.ok]),
    [
      [0, false],
      [1, false],
      [2, true],
    ],
  );
});
