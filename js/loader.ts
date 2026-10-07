/**
 * Loading the WebAssembly module in either environment.
 *
 * The module is built once, with `--target web`. In a browser or worker the
 * generated `init` fetches it. Node is the awkward case: its `fetch` does not
 * handle `file:` URLs, so the bytes are read from disk and handed to `initSync`
 * instead. That single branch is the whole of the "Node adapter".
 */

import type { InitInput } from "../pkg/media_services_wasm.js";

/** The generated bindings, typed loosely: their `.d.ts` ships with the build. */
type Bindings = typeof import("../pkg/media_services_wasm.js");

let bindings: Bindings | undefined;
let loading: Promise<Bindings> | undefined;

function isNode(): boolean {
  return (
    typeof process !== "undefined" &&
    process.versions?.node !== undefined &&
    // Bundlers often polyfill `process` for the browser; the absence of a real
    // filesystem is the better signal.
    typeof process.versions?.node === "string" &&
    process.versions.node.length > 0 &&
    typeof globalThis.window === "undefined"
  );
}

async function nodeInput(): Promise<InitInput> {
  const { readFile } = await import("node:fs/promises");
  const url = new URL("../pkg/media_services_wasm_bg.wasm", import.meta.url);
  const bytes = await readFile(url);
  // Compiling here rather than passing the bytes lets the work overlap with
  // whatever the caller does next.
  return WebAssembly.compile(bytes);
}

/**
 * Loads and initialises the module. Safe to call repeatedly and concurrently;
 * the work happens once.
 *
 * Call this early — the module is a few megabytes, and starting the download
 * before it is needed overlaps it with the rest of application startup.
 */
export async function loadWasm(input?: InitInput): Promise<void> {
  if (bindings) return;
  loading ??= (async () => {
    const module = await import("../pkg/media_services_wasm.js");
    if (input !== undefined) {
      module.initSync({ module: input });
    } else if (isNode()) {
      module.initSync({ module: await nodeInput() });
    } else {
      await module.default();
    }
    bindings = module;
    return module;
  })();

  try {
    await loading;
  } catch (error) {
    // A failed load must not be cached, or every later call fails with the
    // first attempt's error.
    loading = undefined;
    throw error;
  }
}

/** The loaded bindings. Throws if called before {@link loadWasm}. */
export function wasm(): Bindings {
  if (!bindings) {
    throw new Error("the WebAssembly module is not loaded; await loadWasm() first");
  }
  return bindings;
}
