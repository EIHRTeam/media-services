/**
 * The boundary between the generated WebAssembly bindings and this package's API.
 *
 * `Session.process` is typed `Promise<any>` on the bindings' side — nothing in
 * the generated `.d.ts` can describe the object the Rust code builds — so the
 * compiler stops checking the moment the value crosses. This is where it is
 * picked back up, and the failure it guards against is a field renamed on one
 * side only: without it the caller is handed `undefined` where a `Uint8Array`
 * was promised, and finds out somewhere far from the cause.
 *
 * Not reachable from outside the package: `exports` in package.json lists only
 * the entry point, so a subpath import of this file is refused.
 */

import { MediaProvenanceError } from "./errors.js";
import type { ProcessResult } from "./index.js";

/**
 * Narrows what the module returned to the shape this package promises.
 *
 * Returns a fresh object rather than the one it was given. The declared shape is
 * the contract, and copying makes that literal: a field the module adds later is
 * dropped here instead of becoming something a caller quietly depends on. The
 * `bytes` are carried over by reference — it is the caller's image, and copying
 * a multi-megabyte buffer to make a point would be a poor trade.
 */
export function toProcessResult(value: unknown): ProcessResult {
  if (typeof value !== "object" || value === null) {
    throw new MediaProvenanceError(
      "invalidSignerInfo",
      `the pipeline returned ${value === null ? "null" : typeof value}, not a result object`,
    );
  }

  const { bytes, format, xmp } = value as Partial<ProcessResult>;

  // Named one at a time rather than as a list: the message has to say which
  // field is wrong, and the value itself must stay out of it — on this path it
  // is whatever the module produced, and messages travel into logs.
  if (!(bytes instanceof Uint8Array)) {
    throw new MediaProvenanceError(
      "invalidSignerInfo",
      "the pipeline returned no `bytes` as a Uint8Array",
    );
  }
  if (typeof format !== "string") {
    throw new MediaProvenanceError(
      "invalidSignerInfo",
      "the pipeline returned no `format` as a string",
    );
  }
  if (typeof xmp !== "string") {
    throw new MediaProvenanceError(
      "invalidSignerInfo",
      "the pipeline returned no `xmp` as a string",
    );
  }

  return { bytes, format, xmp };
}
