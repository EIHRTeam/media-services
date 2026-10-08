import { MediaProvenanceError, wrap } from "./errors.js";

export interface OperationOptions {
  signal?: AbortSignal;
  /** Per HTTP request, including its response body. Defaults to 30 seconds. */
  requestTimeoutMs?: number;
}

export function aborted(): MediaProvenanceError {
  return new MediaProvenanceError("aborted", "operation aborted");
}

export function checkAbort(signal?: AbortSignal): void {
  if (signal?.aborted) throw aborted();
}

export function positive(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new MediaProvenanceError("invalidArgument", `${name} must be a positive safe integer`);
  }
  return value;
}

export function duration(value: number, name: string): number {
  positive(value, name);
  if (value > 2 ** 31 - 1)
    throw new MediaProvenanceError("invalidArgument", `${name} exceeds the timer range`);
  return value;
}

/** One context per operation, never a mutable context on a shared session. */
export function requestContext(options: OperationOptions = {}) {
  const timeout = duration(options.requestTimeoutMs ?? 30_000, "requestTimeoutMs");
  let failure: MediaProvenanceError | undefined;
  return {
    async fetch(request: Request): Promise<Response> {
      checkAbort(options.signal);
      const controller = new AbortController();
      const cancel = () => {
        failure = aborted();
        controller.abort();
      };
      options.signal?.addEventListener("abort", cancel, { once: true });
      const timer = setTimeout(() => {
        failure = new MediaProvenanceError("timeout", "signing request timed out");
        controller.abort();
      }, timeout);
      try {
        const response = await globalThis.fetch(request, { signal: controller.signal });
        // Keep the deadline active while reading the response body. The signing
        // response is small; its reconstructed Response is consumed by WASM.
        const bytes = await response.arrayBuffer();
        return new Response([204, 205, 304].includes(response.status) ? null : bytes, {
          status: response.status,
          statusText: response.statusText,
          headers: response.headers,
        });
      } finally {
        clearTimeout(timer);
        options.signal?.removeEventListener("abort", cancel);
      }
    },
    error(error: unknown): MediaProvenanceError {
      return options.signal?.aborted ? aborted() : (failure ?? wrap(error));
    },
  };
}

/** Wait without preventing cancellation when a producer is slow. */
export function withAbort<T>(promise: PromiseLike<T>, signal: AbortSignal): Promise<T> {
  checkAbort(signal);
  return new Promise((resolve, reject) => {
    const cancel = () => reject(aborted());
    signal.addEventListener("abort", cancel, { once: true });
    Promise.resolve(promise)
      .then(resolve, reject)
      .finally(() => {
        signal.removeEventListener("abort", cancel);
      });
  });
}
