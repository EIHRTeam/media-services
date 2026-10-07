/**
 * This package's error type, and the classification carried on it.
 *
 * A module of its own rather than part of the entry point: more than one module
 * needs to raise these, and any of them importing `index.ts` for the class would
 * make the entry point and its own helper import each other.
 *
 * The counterpart of `crates/wasm/src/error.rs`, which does the same job on the
 * other side of the boundary.
 */

/** Codes attached to {@link MediaProvenanceError}. */
export type ErrorCode =
  | "unsupportedFormat"
  | "malformedXmp"
  | "containerCorrupt"
  | "signerUnauthorized"
  | "signerRejected"
  | "networkError"
  | "invalidSignerInfo"
  | "malformedManifest"
  | "c2paError"
  | "invalidArgument";

/** An error from the native side, carrying a stable {@link ErrorCode}. */
export class MediaProvenanceError extends Error {
  readonly code: ErrorCode | string;

  constructor(code: string, message: string) {
    super(message);
    this.name = "MediaProvenanceError";
    this.code = code;
  }
}

/** The bindings throw a real `Error` with the classification on `code`. */
export function wrap(error: unknown): MediaProvenanceError {
  if (error instanceof MediaProvenanceError) return error;
  const message = error instanceof Error ? error.message : String(error);
  const code =
    typeof error === "object" && error !== null && "code" in error
      ? String((error as { code: unknown }).code)
      : "unknown";
  return new MediaProvenanceError(code, message);
}
