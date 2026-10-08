import { parentPort, workerData } from "node:worker_threads";
import { connect, loadWasm, readXmp, writeXmp } from "./index.js";
import type { MediaProvenance, ProcessOptions, ConnectOptions, XmpEdit } from "./index.js";
import { MediaProvenanceError, wrap } from "./errors.js";
import { resultBytes } from "./batch.js";
import { wasmMemoryBytes } from "./loader.js";

const port = parentPort!;
let session: MediaProvenance | undefined;
let active: { id: number; controller: AbortController } | undefined;
const fail = (error: unknown) => {
  const wrapped = wrap(error);
  return { code: wrapped.code, message: wrapped.message };
};

try {
  await loadWasm(workerData.wasm as WebAssembly.Module);
  const signing = workerData.signing as ConnectOptions | undefined;
  if (signing) session = await connect(signing);
  port.postMessage({ type: "ready", memoryBytes: wasmMemoryBytes() });
} catch (error) {
  port.postMessage({ type: "initError", error: fail(error) });
  port.close();
}

port.on("message", async (message) => {
  if (message.type === "cancel") {
    if (active && active.id === message.id) active.controller.abort();
    return;
  }
  if (message.type === "close") {
    await session?.close();
    port.close();
    return;
  }
  if (message.type !== "task") return;
  const controller = new AbortController();
  active = { id: message.id, controller };
  try {
    const image = message.image as Uint8Array;
    const controls = {
      signal: controller.signal,
      requestTimeoutMs: message.requestTimeoutMs as number,
    };
    let value: unknown;
    switch (message.operation) {
      case "process":
        if (!session) throw new Error("pool has no signing configuration");
        value = await session.process(image, {
          ...(message.options as ProcessOptions),
          ...controls,
        });
        break;
      case "writeXmp":
        value = await writeXmp(image, message.options as XmpEdit, controls);
        break;
      case "readXmp":
        value = await readXmp(image, controls);
        break;
      default:
        throw new Error("unknown worker operation");
    }
    if (resultBytes(value) > workerData.maxOutputBytes)
      throw new MediaProvenanceError("resourceLimit", "worker output exceeds maxOutputBytes");
    const bytes =
      value instanceof Uint8Array ? value : (value as { bytes?: Uint8Array } | undefined)?.bytes;
    port.postMessage(
      { type: "result", id: message.id, value, memoryBytes: wasmMemoryBytes() },
      bytes ? [bytes.buffer as ArrayBuffer] : [],
    );
  } catch (error) {
    port.postMessage({
      type: "result",
      id: message.id,
      error: fail(error),
      memoryBytes: wasmMemoryBytes(),
    });
  } finally {
    active = undefined;
  }
});
