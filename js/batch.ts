import { MediaProvenanceError, wrap } from "./errors.js";
import { checkAbort, positive, withAbort } from "./controls.js";

export interface LazyImage {
  byteLength: number;
  /** Must honour signal and return exactly byteLength bytes. */
  load(signal: AbortSignal): Promise<Uint8Array>;
}
export type ImageSource = Uint8Array | LazyImage;
export interface BatchItem<O = undefined> {
  id?: string;
  image: ImageSource;
  options?: O;
}
export type BatchSource<O = undefined> = Iterable<BatchItem<O>> | AsyncIterable<BatchItem<O>>;
export type BatchResult<T> = {
  index: number;
  id?: string;
} & ({ ok: true; value: T } | { ok: false; error: MediaProvenanceError });
export interface BatchOptions {
  concurrency?: number;
  maxOutstanding?: number;
  maxInputBytes?: number;
  maxOutputBytes?: number;
  maxInFlightBytes?: number;
  maxBufferedBytes?: number;
  signal?: AbortSignal;
}
const MiB = 1024 * 1024;
export const batchDefaults = Object.freeze({
  concurrency: 2,
  maxOutstanding: 4,
  maxInputBytes: 256 * MiB,
  maxOutputBytes: 320 * MiB,
  maxInFlightBytes: 512 * MiB,
  maxBufferedBytes: 640 * MiB,
});

export function resultBytes(value: unknown): number {
  if (value instanceof Uint8Array) return value.byteLength;
  if (typeof value === "string") return new TextEncoder().encode(value).byteLength;
  if (typeof value === "object" && value !== null && "bytes" in value) {
    const result = value as { bytes: Uint8Array; xmp: string };
    return result.bytes.byteLength + resultBytes(result.xmp);
  }
  return 0;
}

/** Shared scheduler; adapters initialise before entering this generator. */
export async function* runBatch<O, T>(
  source: BatchSource<O>,
  execute: (image: Uint8Array, options: O | undefined, signal: AbortSignal) => Promise<T>,
  options: BatchOptions = {},
): AsyncGenerator<BatchResult<T>> {
  const limits = { ...batchDefaults, ...options };
  for (const key of Object.keys(batchDefaults) as (keyof typeof batchDefaults)[]) {
    positive(limits[key], key);
  }
  if (
    limits.maxInputBytes > limits.maxInFlightBytes ||
    limits.maxOutputBytes > limits.maxBufferedBytes
  ) {
    throw new MediaProvenanceError(
      "invalidArgument",
      "budgets must accommodate a maximum-sized item",
    );
  }
  const iterator =
    Symbol.asyncIterator in source ? source[Symbol.asyncIterator]() : source[Symbol.iterator]();
  const controller = new AbortController();
  const cancel = () => controller.abort();
  options.signal?.addEventListener("abort", cancel, { once: true });
  if (options.signal?.aborted) cancel();
  const signal = controller.signal;
  const completed: { result: BatchResult<T>; size: number }[] = [];
  const active = new Set<Promise<void>>();
  let inputBytes = 0;
  let outputBytes = 0;
  let index = 0;
  let ended = false;
  let pending: BatchItem<O> | undefined;
  let hasPending = false;
  let next: Promise<void> | undefined;
  let sourceError: unknown;
  let sourceFailed = false;
  let wake: (() => void) | undefined;
  const notify = () => wake?.();
  const hasSlot = () =>
    active.size < limits.concurrency &&
    active.size + completed.length < limits.maxOutstanding &&
    outputBytes + limits.maxOutputBytes <= limits.maxBufferedBytes;
  const pull = () => {
    next = Promise.resolve()
      .then(() => iterator.next())
      .then((item) => {
        if (typeof item !== "object" || item === null)
          throw new MediaProvenanceError("invalidArgument", "iterator returned no result object");
        ended = !!item.done;
        if (!item.done) {
          pending = item.value;
          hasPending = true;
        }
      })
      .catch((error: unknown) => {
        sourceError = error;
        sourceFailed = true;
      })
      .finally(() => {
        next = undefined;
        notify();
      });
  };
  const launch = (item: BatchItem<O>, length: number) => {
    const position = index++;
    const identity = { index: position, ...(item?.id === undefined ? {} : { id: item.id }) };
    inputBytes += length;
    outputBytes += limits.maxOutputBytes;
    const task = (async () => {
      let size = 0;
      let result: BatchResult<T>;
      try {
        checkAbort(signal);
        if (!item || !item.image)
          throw new MediaProvenanceError("invalidArgument", "batch item requires an image");
        const declared = item.image.byteLength;
        if (!Number.isSafeInteger(declared) || declared < 0 || declared > limits.maxInputBytes) {
          throw new MediaProvenanceError(
            "resourceLimit",
            "input exceeds maxInputBytes or has an invalid byteLength",
          );
        }
        const image = item.image instanceof Uint8Array ? item.image : await item.image.load(signal);
        checkAbort(signal);
        if (!(image instanceof Uint8Array) || image.byteLength !== length) {
          throw new MediaProvenanceError(
            "invalidArgument",
            "loaded image does not match declared byteLength",
          );
        }
        const value = await execute(image, item.options, signal);
        checkAbort(signal);
        size = resultBytes(value);
        if (size > limits.maxOutputBytes) {
          size = 0;
          throw new MediaProvenanceError("resourceLimit", "output exceeds maxOutputBytes");
        }
        result = { ...identity, ok: true, value };
      } catch (error) {
        result = { ...identity, ok: false, error: wrap(error) };
      } finally {
        inputBytes -= length;
      }
      outputBytes -= limits.maxOutputBytes - size;
      completed.push({ result, size });
    })();
    active.add(task);
    void task.finally(() => {
      active.delete(task);
      notify();
    });
  };
  try {
    while (true) {
      checkAbort(signal);
      if (sourceFailed) throw sourceError;
      if (completed.length) {
        const done = completed.shift()!;
        outputBytes -= done.size;
        yield done.result;
        continue;
      }
      if (hasPending && hasSlot()) {
        const length = pending?.image?.byteLength ?? NaN;
        const valid = Number.isSafeInteger(length) && length >= 0 && length <= limits.maxInputBytes;
        if (!valid || inputBytes + length <= limits.maxInFlightBytes) {
          const item = pending as BatchItem<O>;
          pending = undefined;
          hasPending = false;
          // Invalid tasks are isolated without reserving their untrusted size.
          launch(item, valid ? length : 0);
          continue;
        }
      }
      if (!ended && !hasPending && !next && hasSlot()) {
        pull();
        continue;
      }
      if (ended && !hasPending && !next && !active.size) break;
      await withAbort(
        new Promise<void>((resolve) => {
          wake = resolve;
        }),
        signal,
      );
      wake = undefined;
    }
  } finally {
    controller.abort();
    options.signal?.removeEventListener("abort", cancel);
    // The producer and lazy loaders must cooperate with cancellation. Await
    // running work before releasing native sessions or their input storage.
    await Promise.allSettled([...active, ...(next ? [next] : [])]);
    await iterator.return?.();
    completed.length = 0;
    pending = undefined;
  }
}
