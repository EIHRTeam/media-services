#!/usr/bin/env node
/** Fresh process per scenario: RSS high-water marks never cross-contaminate. */
import { fork } from "node:child_process";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { monitorEventLoopDelay, performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const MiB = 1024 * 1024;
if (process.argv[2] === "--scenario") {
  const config = JSON.parse(process.argv[3]);
  const startupStart = performance.now();
  const { connect, loadWasm, writeXmp, writeXmpBatch } = await import("../dist/index.js");
  const { createWorkerPool } = await import("../dist/node.js");
  const { wasmMemoryBytes } = await import("../dist/loader.js");
  const { startMockSigner } = await import("../test/mock-signer.mjs");
  const base = await readFile(
    join(
      root,
      config.format === "webp" ? "target/tmp/plain.webp" : "crates/xmp-image/tests/base.avif",
    ),
  );
  const size = config.mib * MiB;
  const makeImage = () => {
    const bytes = new Uint8Array(size);
    bytes.set(base);
    const view = new DataView(bytes.buffer);
    if (config.format === "webp") {
      bytes.set(Buffer.from("JUNK"), base.length);
      view.setUint32(base.length + 4, (size - base.length - 8) & ~1, true);
      view.setUint32(4, size - 8, true);
    } else {
      view.setUint32(base.length, size - base.length);
      bytes.set(Buffer.from("free"), base.length + 4);
    }
    return bytes;
  };
  const token = "benchmark-throwaway-token";
  const service = config.operation === "process" ? await startMockSigner({ token }) : undefined;
  const signing = service ? { signerEndpoint: service.endpoint, token } : undefined;
  const pool = config.workers
    ? await createWorkerPool({
        workers: config.workers,
        ...(signing ? { signing } : {}),
        maxOutputBytes: size + 2 * MiB,
      })
    : undefined;
  const session = !pool && signing ? await connect(signing) : undefined;
  const edit = { creatorTool: "Performance benchmark" };
  const warmOptions = {
    title: `warmup.${config.format}`,
    manifest: {
      claim_version: 2,
      claim_generator_info: [{ name: "Benchmark" }],
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
    xmp: edit,
  };
  if (pool)
    await Promise.all(
      Array.from({ length: config.workers }, () =>
        config.operation === "process"
          ? pool.process(base, warmOptions)
          : pool.writeXmp(base, edit),
      ),
    );
  else {
    await loadWasm();
    if (session) await session.process(base, warmOptions);
    else await writeXmp(base, edit);
  }
  const startupMs = performance.now() - startupStart;
  const source = function* () {
    for (let index = 0; index < config.items; index++)
      yield {
        image: { byteLength: size, load: async () => makeImage() },
        options:
          config.operation === "process"
            ? {
                title: `benchmark.${config.format}`,
                manifest: {
                  claim_version: 2,
                  claim_generator_info: [{ name: "Benchmark" }],
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
                },
                xmp: edit,
              }
            : edit,
      };
  };
  const options = {
    concurrency: config.workers || 2,
    transfer: true,
    maxOutputBytes: size + 2 * MiB,
    maxBufferedBytes: 640 * MiB,
    maxInFlightBytes: 512 * MiB,
  };
  const histogram = monitorEventLoopDelay({ resolution: 1 });
  let peakRss = 0;
  let peakExternal = 0;
  let peakArrayBuffers = 0;
  let peakWasm = 0;
  const sample = () => {
    const memory = process.memoryUsage();
    peakRss = Math.max(peakRss, memory.rss);
    peakExternal = Math.max(peakExternal, memory.external);
    peakArrayBuffers = Math.max(peakArrayBuffers, memory.arrayBuffers);
    peakWasm = Math.max(peakWasm, pool ? pool.stats.wasmMemoryBytes : wasmMemoryBytes());
  };
  let successes = 0;
  const errors = [];
  histogram.enable();
  await delay(10);
  const timer = setInterval(sample, 2);
  const start = performance.now();
  try {
    const results =
      config.operation === "process"
        ? (pool ?? session).processBatch(source(), options)
        : pool
          ? pool.writeXmpBatch(source(), options)
          : writeXmpBatch(source(), options);
    for await (const result of results) {
      sample();
      if (result.ok) successes++;
      else errors.push({ code: result.error.code, message: result.error.message });
    }
    const elapsedMs = performance.now() - start;
    await delay(10);
    histogram.disable();
    sample();
    const record = {
      ...config,
      startupMs,
      successes,
      errors,
      elapsedMs,
      imagesPerSecond: (successes * 1000) / elapsedMs,
      inputMiBPerSecond: (successes * config.mib * 1000) / elapsedMs,
      eventLoopP95Ms: histogram.percentile(95) / 1e6,
      eventLoopMaxMs: histogram.max / 1e6,
      peakRssBytes: Math.max(peakRss, process.resourceUsage().maxRSS * 1024),
      peakExternalBytes: peakExternal,
      peakArrayBufferBytes: peakArrayBuffers,
      peakWasmBytes: peakWasm,
    };
    process.send(record);
  } finally {
    clearInterval(timer);
    histogram.disable();
    await pool?.close();
    await session?.close();
    await service?.close();
  }
} else {
  const sizes = (process.env.MPS_BENCH_SIZES ?? "1,32,128,256").split(",").map(Number);
  const items = Number(process.env.MPS_BENCH_ITEMS ?? 4);
  const records = [];
  for (const mib of sizes)
    for (const format of ["webp", "avif"])
      for (const operation of ["writeXmp", "process"])
        for (const workers of [0, 1, 2, 4]) {
          const config = { mib, format, operation, workers, items };
          let record;
          const child = fork(
            fileURLToPath(import.meta.url),
            ["--scenario", JSON.stringify(config)],
            { stdio: ["ignore", "inherit", "inherit", "ipc"], execArgv: [] },
          );
          child.on("message", (value) => {
            record = value;
          });
          await new Promise((resolve, reject) => {
            child.on("error", reject);
            child.on("exit", (code) =>
              code === 0 && record
                ? resolve()
                : reject(
                    new Error(`benchmark child failed: ${JSON.stringify(config)} (exit ${code})`),
                  ),
            );
          });
          records.push(record);
          console.log(JSON.stringify(record));
        }
  await mkdir(join(root, "target/tmp"), { recursive: true });
  await writeFile(
    join(root, "target/tmp/benchmark.json"),
    JSON.stringify({ node: process.version, platform: process.platform, records }, null, 2),
  );
  if (records.some((record) => record.errors.length)) process.exitCode = 1;
}
