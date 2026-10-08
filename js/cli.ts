#!/usr/bin/env node
import { parseArgs } from "node:util";
import { readFile, writeFile, lstat, opendir, mkdir, rename, link, unlink } from "node:fs/promises";
import { basename, dirname, extname, join, relative, resolve, isAbsolute } from "node:path";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { connect, readXmpBatch, writeXmpBatch } from "./index.js";
import type { MediaProvenance, XmpEdit, ManifestDefinition, ProcessOptions } from "./index.js";
import { createWorkerPool } from "./node.js";
import type { MediaWorkerPool, WorkerBatchOptions } from "./node.js";
import type { BatchItem } from "./batch.js";
import { checkAbort, positive } from "./controls.js";
import { MediaProvenanceError, wrap } from "./errors.js";

const help = `mps — XMP metadata and C2PA signing (Node.js >=24)

Usage:
  mps xmp read <files/directories...> [--recursive] [--json]
  mps xmp write <files/directories...> --edit edit.json [output options]
  mps sign <files/directories...> --manifest manifest.json [output options]

Output: --output FILE (single file), --output-dir DIR (batch), --force
XMP: --edit FILE (XmpEdit JSON), --base-packet FILE (XML, used if no XMP exists)
Signing: --signer-endpoint URL or SIGNER_ENDPOINT; SIGNER_TOKEN or --token-file FILE
Execution: --workers N (opt-in), --concurrency N, --recursive, --json
Budgets (bytes, or KiB/MiB/GiB): --max-input-bytes, --max-output-bytes,
  --max-in-flight-bytes, --max-buffered-bytes; --max-outstanding N
Network: --request-timeout-ms N (default 30000)
Other: --help, --version

npx @eihrteam/mps-worker xmp read input.webp
npx --package=@eihrteam/mps-worker mps sign image.jpg --manifest claim.json --output signed.jpg
`;

function bytes(value: string, name: string): number {
  const match = /^(\d+)(KiB|MiB|GiB)?$/i.exec(value);
  if (!match)
    throw new MediaProvenanceError(
      "invalidArgument",
      `${name} must be bytes or an integer with KiB/MiB/GiB suffix`,
    );
  const exponent = { kib: 1, mib: 2, gib: 3 }[match[2]?.toLowerCase() ?? ""] ?? 0;
  return positive(Number(match[1]) * 1024 ** exponent, name);
}
function inside(directory: string, file: string): boolean {
  const path = relative(directory, file);
  return (
    path === "" ||
    (!path.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) &&
      path !== ".." &&
      !isAbsolute(path))
  );
}
async function emit(stream: NodeJS.WriteStream, text: string): Promise<void> {
  if (!stream.write(text)) await once(stream, "drain");
}
async function atomicWrite(
  target: string,
  value: Uint8Array,
  force: boolean,
  signal: AbortSignal,
): Promise<void> {
  checkAbort(signal);
  await mkdir(dirname(target), { recursive: true });
  const temporary = join(dirname(target), `.mps-${randomUUID()}.tmp`);
  try {
    await writeFile(temporary, value, { flag: "wx", signal });
    checkAbort(signal);
    if (force) await rename(temporary, target);
    else await link(temporary, target); // Atomic no-clobber; a preflight exists check races.
  } finally {
    await unlink(temporary).catch((error: NodeJS.ErrnoException) => {
      if (error.code !== "ENOENT") throw error;
    });
  }
}

async function main(): Promise<number> {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      help: { type: "boolean", short: "h" },
      version: { type: "boolean", short: "v" },
      recursive: { type: "boolean" },
      json: { type: "boolean" },
      force: { type: "boolean" },
      output: { type: "string", short: "o" },
      "output-dir": { type: "string" },
      edit: { type: "string" },
      "base-packet": { type: "string" },
      manifest: { type: "string" },
      "signer-endpoint": { type: "string" },
      "token-file": { type: "string" },
      workers: { type: "string" },
      concurrency: { type: "string" },
      "max-input-bytes": { type: "string" },
      "max-output-bytes": { type: "string" },
      "max-in-flight-bytes": { type: "string" },
      "max-buffered-bytes": { type: "string" },
      "max-outstanding": { type: "string" },
      "request-timeout-ms": { type: "string" },
    },
  });
  if (values.help) {
    await emit(process.stdout, help);
    return 0;
  }
  if (values.version) {
    const pkg = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
    await emit(process.stdout, `${pkg.version}\n`);
    return 0;
  }
  let command = positionals.shift();
  if (command === "xmp") command = positionals.shift();
  else if (command !== "sign") throw new Error("expected sign, xmp read or xmp write; see --help");
  if (!["read", "write", "sign"].includes(command ?? ""))
    throw new Error("expected xmp read or xmp write; see --help");
  if (!positionals.length) throw new Error("at least one input is required");
  if (values.output && values["output-dir"]) throw new Error("choose --output or --output-dir");
  if (command === "read" && (values.output || values["output-dir"]))
    throw new Error("xmp read writes to stdout");
  const roots = await Promise.all(
    positionals.map(async (file) => {
      const path = resolve(file);
      const info = await lstat(path);
      if (info.isSymbolicLink() || (!info.isDirectory() && !info.isFile()))
        throw new Error(`input is not a regular file or directory: ${path}`);
      return { path, directory: info.isDirectory() };
    }),
  );
  const single = roots.length === 1 && !roots[0]!.directory;
  if (command !== "read" && !(single ? values.output : values["output-dir"])) {
    throw new Error(
      single ? "writing a file requires --output" : "batch writing requires --output-dir",
    );
  }
  if (!single && values.output) throw new Error("--output requires one regular input file");
  if (command === "write" && !values.edit && !values["base-packet"])
    throw new Error("xmp write requires --edit or --base-packet");
  if (command === "sign" && !values.manifest) throw new Error("sign requires --manifest");
  const parseObject = async (file: string) => {
    const value: unknown = JSON.parse(await readFile(file, "utf8"));
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new Error(`configuration must be a JSON object: ${file}`);
    return value;
  };
  const edit = (values.edit ? await parseObject(values.edit) : {}) as XmpEdit;
  if (values["base-packet"]) edit.basePacket = await readFile(values["base-packet"], "utf8");
  const manifest = values.manifest
    ? ((await parseObject(values.manifest)) as ManifestDefinition)
    : undefined;
  const workers =
    values.workers === undefined || values.workers === "0"
      ? 0
      : positive(Number(values.workers), "workers");
  const requestTimeoutMs = positive(
    Number(values["request-timeout-ms"] ?? 30_000),
    "request-timeout-ms",
  );
  const controller = new AbortController();
  const interrupt = () => controller.abort();
  process.on("SIGINT", interrupt);
  const budgets: WorkerBatchOptions = { signal: controller.signal, transfer: true };
  const parameters = {
    "max-input-bytes": "maxInputBytes",
    "max-output-bytes": "maxOutputBytes",
    "max-in-flight-bytes": "maxInFlightBytes",
    "max-buffered-bytes": "maxBufferedBytes",
  } as const;
  for (const [flag, key] of Object.entries(parameters)) {
    const value = values[flag as keyof typeof parameters];
    if (value !== undefined) budgets[key] = bytes(value, flag);
  }
  if (values["max-outstanding"])
    budgets.maxOutstanding = positive(Number(values["max-outstanding"]), "max-outstanding");
  if (values.concurrency) budgets.concurrency = positive(Number(values.concurrency), "concurrency");
  else if (workers) budgets.concurrency = workers;
  const outputDirectory = values["output-dir"] ? resolve(values["output-dir"]) : undefined;
  const supported = new Set([".jpg", ".jpeg", ".png", ".webp", ".avif"]);
  const directories = roots.filter((root) => root.directory).length;
  const discover = async function* (directory: string): AsyncGenerator<string> {
    checkAbort(controller.signal);
    const entries = await opendir(directory);
    for await (const entry of entries) {
      checkAbort(controller.signal);
      const path = join(directory, entry.name);
      if (outputDirectory && inside(outputDirectory, path)) continue;
      if (entry.isDirectory()) {
        if (values.recursive) yield* discover(path);
      } else if (entry.isFile() && supported.has(extname(path).toLowerCase())) yield path;
    }
  };
  let seen = 0;
  const inputs = async function* <O>(
    optionsFor: (path: string) => O,
  ): AsyncGenerator<BatchItem<O>> {
    for (const [rootIndex, root] of roots.entries()) {
      const paths = root.directory ? discover(root.path) : [root.path];
      for await (const path of paths) {
        checkAbort(controller.signal);
        const info = await lstat(path);
        if (!info.isFile() || info.isSymbolicLink()) continue;
        let target = values.output ? resolve(values.output) : undefined;
        if (outputDirectory) {
          const subdirectory =
            root.directory && directories > 1 ? `${rootIndex + 1}-${basename(root.path)}` : "";
          target = join(
            outputDirectory,
            subdirectory,
            root.directory ? relative(root.path, path) : basename(path),
          );
        }
        seen++;
        yield {
          id: JSON.stringify({ input: path, target }),
          image: {
            byteLength: info.size,
            load: async (signal) => {
              // Allocate an exclusive backing store, safe for transfer even for
              // tiny files (fs.readFile may use the Node Buffer pool).
              return Uint8Array.from(await readFile(path, { signal }));
            },
          },
          options: optionsFor(path),
        };
      }
    }
  };
  let session: MediaProvenance | undefined;
  let pool: MediaWorkerPool | undefined;
  let failed = false;
  try {
    const signing =
      command === "sign"
        ? {
            signerEndpoint: values["signer-endpoint"] ?? process.env.SIGNER_ENDPOINT ?? "",
            token: values["token-file"]
              ? (await readFile(values["token-file"], "utf8")).trim()
              : (process.env.SIGNER_TOKEN ?? ""),
            requestTimeoutMs,
          }
        : undefined;
    if (workers) {
      pool = await createWorkerPool({
        workers,
        requestTimeoutMs,
        signal: controller.signal,
        ...(signing ? { signing } : {}),
        ...(budgets.maxInputBytes === undefined ? {} : { maxInputBytes: budgets.maxInputBytes }),
        ...(budgets.maxOutputBytes === undefined ? {} : { maxOutputBytes: budgets.maxOutputBytes }),
        ...(budgets.maxInFlightBytes === undefined
          ? {}
          : { maxInFlightBytes: budgets.maxInFlightBytes }),
      });
    } else if (signing) session = await connect({ ...signing, signal: controller.signal });
    const results =
      command === "sign"
        ? (pool ?? session!).processBatch(
            inputs<ProcessOptions>((path) => ({
              manifest: manifest!,
              title: basename(path),
              xmp: edit,
            })),
            budgets,
          )
        : command === "write"
          ? pool
            ? pool.writeXmpBatch(
                inputs(() => edit),
                budgets,
              )
            : writeXmpBatch(
                inputs(() => edit),
                budgets,
              )
          : pool
            ? pool.readXmpBatch(
                inputs(() => undefined),
                budgets,
              )
            : readXmpBatch(
                inputs(() => undefined),
                budgets,
              );
    for await (const item of results) {
      checkAbort(controller.signal);
      const identity = JSON.parse(item.id!) as { input: string; target?: string };
      try {
        if (!item.ok) throw item.error;
        const value = item.value;
        if (command === "read") {
          if (single && !values.json)
            await emit(process.stdout, value === undefined ? "" : `${value as string}\n`);
          else
            await emit(
              process.stdout,
              `${JSON.stringify({ index: item.index, input: identity.input, ok: true, xmp: value ?? null })}\n`,
            );
        } else {
          const result = value as Uint8Array | { bytes: Uint8Array; format: string };
          await atomicWrite(
            identity.target!,
            result instanceof Uint8Array ? result : result.bytes,
            !!values.force,
            controller.signal,
          );
          const record = { index: item.index, ...identity, ok: true };
          if (values.json) await emit(process.stdout, `${JSON.stringify(record)}\n`);
          else await emit(process.stderr, `${identity.input} -> ${identity.target}\n`);
        }
      } catch (error) {
        checkAbort(controller.signal);
        failed = true;
        const problem = wrap(error);
        if (values.json || (command === "read" && !single)) {
          await emit(
            process.stdout,
            `${JSON.stringify({ index: item.index, ...identity, ok: false, error: { code: problem.code, message: problem.message } })}\n`,
          );
        } else
          await emit(process.stderr, `${identity.input}: ${problem.code}: ${problem.message}\n`);
      }
    }
    if (!seen) throw new Error("no supported input files found");
    return failed ? 1 : 0;
  } catch (error) {
    if (controller.signal.aborted) return 130;
    throw error;
  } finally {
    await pool?.close();
    await session?.close();
    process.removeListener("SIGINT", interrupt);
  }
}

try {
  process.exitCode = await main();
} catch (error) {
  const problem = wrap(error);
  console.error(`mps: ${problem.code}: ${problem.message}`);
  process.exitCode = 2;
}
