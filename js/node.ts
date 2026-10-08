import { Worker, isMarkedAsUntransferable } from "node:worker_threads";
import { availableParallelism } from "node:os";
import { readFile } from "node:fs/promises";
import { AsyncResource } from "node:async_hooks";
import { MediaProvenanceError, wrap } from "./errors.js";
import { aborted, checkAbort, duration, positive, withAbort } from "./controls.js";
import type { OperationOptions } from "./controls.js";
import { batchDefaults, resultBytes, runBatch } from "./batch.js";
import type { BatchSource, BatchOptions, BatchResult } from "./batch.js";
import type { ConnectOptions, ProcessOptions, ProcessResult, XmpEdit } from "./index.js";

export interface WorkerPoolOptions {
  workers?: number;
  signing?: Pick<ConnectOptions, "signerEndpoint" | "token" | "requestTimeoutMs">;
  wasm?: WebAssembly.Module | BufferSource;
  maxQueuedTasks?: number;
  maxInputBytes?: number;
  maxOutputBytes?: number;
  maxInFlightBytes?: number;
  closeTimeoutMs?: number;
  requestTimeoutMs?: number;
  signal?: AbortSignal;
}
export interface WorkerBatchOptions extends BatchOptions {
  transfer?: boolean;
}
export interface WorkerOperationOptions extends OperationOptions {
  /** Detaches a complete, transferable, exclusively owned ArrayBuffer. */
  transfer?: boolean;
}
type Operation = "process" | "readXmp" | "writeXmp";
interface Task {
  id: number;
  byteLength: number;
  image: Uint8Array;
  operation: Operation;
  options: unknown;
  timeout: number;
  signal?: AbortSignal;
  cancel: () => void;
  resource: AsyncResource;
  resolve: (value: unknown) => void;
  reject: (error: unknown) => void;
}
interface Slot {
  worker: Worker;
  ready: boolean;
  task?: Task;
  memoryBytes: number;
  initialised: boolean;
}

export class MediaWorkerPool {
  readonly size: number;
  readonly #options: Required<
    Pick<
      WorkerPoolOptions,
      | "maxQueuedTasks"
      | "maxInputBytes"
      | "maxOutputBytes"
      | "maxInFlightBytes"
      | "closeTimeoutMs"
      | "requestTimeoutMs"
    >
  >;
  readonly #wasm: WebAssembly.Module;
  readonly #signing: ConnectOptions | undefined;
  readonly #slots = new Set<Slot>();
  readonly #queue: Task[] = [];
  readonly #waiters = new Set<() => void>();
  readonly #batches = new Set<AbortController>();
  #bytes = 0;
  #id = 0;
  #closing = false;
  #fatal: MediaProvenanceError | undefined;
  #close: Promise<void> | undefined;

  /** @internal Use createWorkerPool. */
  constructor(wasm: WebAssembly.Module, options: WorkerPoolOptions) {
    this.#wasm = wasm;
    this.size = positive(
      options.workers ?? Math.min(4, Math.max(1, availableParallelism() - 1)),
      "workers",
    );
    this.#options = {
      maxQueuedTasks: positive(options.maxQueuedTasks ?? 4, "maxQueuedTasks"),
      maxInputBytes: positive(
        options.maxInputBytes ?? batchDefaults.maxInputBytes,
        "maxInputBytes",
      ),
      maxOutputBytes: positive(
        options.maxOutputBytes ?? batchDefaults.maxOutputBytes,
        "maxOutputBytes",
      ),
      maxInFlightBytes: positive(
        options.maxInFlightBytes ?? batchDefaults.maxInFlightBytes,
        "maxInFlightBytes",
      ),
      closeTimeoutMs: duration(options.closeTimeoutMs ?? 5000, "closeTimeoutMs"),
      requestTimeoutMs: duration(
        options.requestTimeoutMs ?? options.signing?.requestTimeoutMs ?? 30_000,
        "requestTimeoutMs",
      ),
    };
    if (this.#options.maxInputBytes > this.#options.maxInFlightBytes) {
      throw new MediaProvenanceError(
        "invalidArgument",
        "pool budget must accommodate maxInputBytes",
      );
    }
    this.#signing = options.signing
      ? {
          signerEndpoint: options.signing.signerEndpoint,
          token: options.signing.token,
          requestTimeoutMs: this.#options.requestTimeoutMs,
        }
      : undefined;
  }

  get stats() {
    return {
      workers: this.#slots.size,
      active: [...this.#slots].filter((slot) => slot.task).length,
      queued: this.#queue.length,
      inputBytes: this.#bytes,
      wasmMemoryBytes: [...this.#slots].reduce((total, slot) => total + slot.memoryBytes, 0),
    };
  }

  async initialise(signal?: AbortSignal): Promise<void> {
    try {
      const startup = Promise.all(Array.from({ length: this.size }, () => this.#spawn()));
      if (signal) await withAbort(startup, signal);
      else await startup;
    } catch (error) {
      await this.close();
      throw wrap(error);
    }
  }

  #notify(): void {
    for (const resolve of this.#waiters) resolve();
    this.#waiters.clear();
  }

  #spawn(): Promise<void> {
    const worker = new Worker(new URL("./worker.js", import.meta.url), {
      // These are compiled file-backed workers. Process/eval/test-runner flags
      // can be invalid for Worker startup; use its own runtime defaults.
      execArgv: [],
      workerData: {
        wasm: this.#wasm,
        signing: this.#signing,
        maxOutputBytes: this.#options.maxOutputBytes,
      },
    });
    const slot: Slot = { worker, ready: false, memoryBytes: 0, initialised: false };
    this.#slots.add(slot);
    return new Promise((resolve, reject) => {
      let lost = false;
      const failure = (error: unknown) => {
        if (lost) return;
        lost = true;
        this.#slots.delete(slot);
        const problem = wrap(error);
        if (slot.task) this.#finish(slot, problem);
        reject(problem);
        void worker.terminate();
        this.#notify();
        if (!this.#closing && slot.initialised) {
          void Promise.resolve()
            .then(() => this.#spawn())
            .catch((cause: unknown) => {
              this.#fatal = wrap(cause);
              void this.close();
            });
        }
      };
      worker.on("error", (error) =>
        failure(new MediaProvenanceError("workerError", error.message)),
      );
      worker.on("exit", (code) => {
        if (!this.#closing || slot.task || !slot.initialised) {
          failure(new MediaProvenanceError("workerError", `worker exited with code ${code}`));
        } else {
          this.#slots.delete(slot);
          this.#notify();
        }
      });
      worker.on("message", (message) => {
        if (lost) return;
        slot.memoryBytes = message.memoryBytes ?? slot.memoryBytes;
        if (message.type === "initError") {
          failure(new MediaProvenanceError(message.error.code, message.error.message));
        } else if (message.type === "ready") {
          slot.initialised = true;
          slot.ready = true;
          resolve();
          this.#dispatch();
        } else if (message.type === "result" && slot.task?.id === message.id) {
          let error = message.error
            ? new MediaProvenanceError(message.error.code, message.error.message)
            : undefined;
          if (slot.task?.signal?.aborted) error = aborted();
          if (!error && resultBytes(message.value) > this.#options.maxOutputBytes) {
            error = new MediaProvenanceError(
              "resourceLimit",
              "worker output exceeds maxOutputBytes",
            );
          }
          this.#finish(slot, error, message.value);
          this.#dispatch();
        }
      });
    });
  }

  #settle(task: Task, error?: unknown, value?: unknown): void {
    this.#bytes -= task.byteLength;
    task.signal?.removeEventListener("abort", task.cancel);
    task.resource.runInAsyncScope(() => (error ? task.reject(error) : task.resolve(value)));
    task.resource.emitDestroy();
    this.#notify();
  }

  #finish(slot: Slot, error?: unknown, value?: unknown): void {
    const task = slot.task!;
    delete slot.task;
    slot.ready = true;
    this.#settle(task, error, value);
  }

  #dispatch(): void {
    if (this.#closing) return;
    for (const slot of this.#slots) {
      while (slot.ready && !slot.task) {
        const task = this.#queue.shift();
        if (!task) break;
        slot.task = task;
        slot.ready = false;
        try {
          slot.worker.postMessage(
            {
              type: "task",
              id: task.id,
              operation: task.operation,
              image: task.image,
              options: task.options,
              requestTimeoutMs: task.timeout,
            },
            [task.image.buffer as ArrayBuffer],
          );
        } catch (error) {
          this.#finish(slot, wrap(error));
        }
      }
    }

    this.#notify();
  }

  #assertOpen(): void {
    if (this.#fatal) throw this.#fatal;
    if (this.#closing) throw new MediaProvenanceError("invalidArgument", "worker pool is closed");
  }

  #capacity(length: number): boolean {
    const idle = [...this.#slots].some((slot) => slot.ready && !slot.task);
    return (
      (idle || this.#queue.length < this.#options.maxQueuedTasks) &&
      this.#bytes + length <= this.#options.maxInFlightBytes
    );
  }

  #submit(
    operation: Operation,
    image: Uint8Array,
    options: unknown,
    controls: WorkerOperationOptions = {},
  ): Promise<unknown> {
    this.#assertOpen();
    checkAbort(controls.signal);
    if (operation === "process" && !this.#signing) {
      throw new MediaProvenanceError("invalidArgument", "signing configuration is required");
    }
    if (!(image instanceof Uint8Array))
      throw new MediaProvenanceError("invalidArgument", "image must be a Uint8Array");
    if (image.byteLength > this.#options.maxInputBytes)
      throw new MediaProvenanceError("resourceLimit", "worker input exceeds maxInputBytes");
    if (!this.#capacity(image.byteLength))
      throw new MediaProvenanceError("queueFull", "worker queue or byte budget is full");
    const timeout = duration(
      controls.requestTimeoutMs ?? this.#options.requestTimeoutMs,
      "requestTimeoutMs",
    );
    if (
      controls.transfer &&
      (!(image.buffer instanceof ArrayBuffer) ||
        image.byteOffset !== 0 ||
        image.byteLength !== image.buffer.byteLength ||
        isMarkedAsUntransferable(image.buffer))
    ) {
      throw new MediaProvenanceError(
        "invalidArgument",
        "transfer requires a complete transferable ArrayBuffer with exclusive ownership",
      );
    }
    // structuredClone detaches immediately on accepted transfer, including
    // queued tasks. The pool then owns this exact-sized backing store.
    const owned = controls.transfer
      ? structuredClone(image, { transfer: [image.buffer as ArrayBuffer] })
      : Uint8Array.from(image);
    this.#bytes += owned.byteLength;
    return new Promise((resolve, reject) => {
      const task: Task = {
        id: this.#id++,
        byteLength: owned.byteLength,
        image: owned,
        operation,
        options,
        timeout,
        ...(controls.signal ? { signal: controls.signal } : {}),
        resource: new AsyncResource("MediaWorkerTask"),
        resolve,
        reject,
        cancel: () => {
          const queued = this.#queue.indexOf(task);
          if (queued >= 0) {
            this.#queue.splice(queued, 1);
            this.#settle(task, aborted());
            this.#dispatch();
          } else {
            const slot = [...this.#slots].find((entry) => entry.task === task);
            slot?.worker.postMessage({ type: "cancel", id: task.id });
          }
        },
      };
      controls.signal?.addEventListener("abort", task.cancel, { once: true });
      this.#queue.push(task);
      this.#dispatch();
    });
  }

  async process(
    image: Uint8Array,
    options: ProcessOptions,
    controls: WorkerOperationOptions = {},
  ): Promise<ProcessResult> {
    const { signal, requestTimeoutMs, ...edit } = options;
    return (await this.#submit("process", image, edit, {
      ...controls,
      ...(signal || controls.signal
        ? {
            signal:
              signal && controls.signal
                ? AbortSignal.any([signal, controls.signal])
                : (signal ?? controls.signal)!,
          }
        : {}),
      ...(requestTimeoutMs === undefined ? {} : { requestTimeoutMs }),
    })) as ProcessResult;
  }
  async readXmp(
    image: Uint8Array,
    controls: WorkerOperationOptions = {},
  ): Promise<string | undefined> {
    return (await this.#submit("readXmp", image, undefined, controls)) as string | undefined;
  }
  async writeXmp(
    image: Uint8Array,
    edit: XmpEdit = {},
    controls: WorkerOperationOptions = {},
  ): Promise<Uint8Array> {
    return (await this.#submit("writeXmp", image, edit, controls)) as Uint8Array;
  }

  async #waitForCapacity(length: number, signal: AbortSignal): Promise<void> {
    while (true) {
      this.#assertOpen();
      checkAbort(signal);
      if (this.#capacity(length)) return;
      let notify!: () => void;
      try {
        await withAbort(
          new Promise<void>((resolve) => {
            notify = resolve;
            this.#waiters.add(resolve);
          }),
          signal,
        );
      } finally {
        this.#waiters.delete(notify);
      }
    }
  }

  #batch<O, T>(
    source: BatchSource<O>,
    execute: (
      image: Uint8Array,
      options: O | undefined,
      controls: WorkerOperationOptions,
    ) => Promise<T>,
    options: WorkerBatchOptions,
  ): AsyncGenerator<BatchResult<T>> {
    this.#assertOpen();
    return this.#iterateBatch(source, execute, options);
  }

  async *#iterateBatch<O, T>(
    source: BatchSource<O>,
    execute: (
      image: Uint8Array,
      options: O | undefined,
      controls: WorkerOperationOptions,
    ) => Promise<T>,
    options: WorkerBatchOptions,
  ): AsyncGenerator<BatchResult<T>> {
    this.#assertOpen();
    const controller = new AbortController();
    const signal = options.signal
      ? AbortSignal.any([options.signal, controller.signal])
      : controller.signal;
    this.#batches.add(controller);
    try {
      yield* runBatch(
        source,
        async (image, edit, signal) => {
          if (image.byteLength > this.#options.maxInputBytes)
            throw new MediaProvenanceError("resourceLimit", "worker input exceeds maxInputBytes");
          while (true) {
            await this.#waitForCapacity(image.byteLength, signal);
            try {
              return await execute(image, edit, { signal, transfer: options.transfer ?? false });
            } catch (error) {
              if (wrap(error).code !== "queueFull") throw error;
            }
          }
        },
        { concurrency: this.size, ...options, signal },
      );
    } finally {
      this.#batches.delete(controller);
    }
  }

  processBatch(
    source: BatchSource<ProcessOptions>,
    options: WorkerBatchOptions = {},
  ): AsyncGenerator<BatchResult<ProcessResult>> {
    return this.#batch(
      source,
      (image, edit, controls) => {
        if (!edit)
          throw new MediaProvenanceError("invalidArgument", "process options are required");
        return this.process(image, edit, controls);
      },
      options,
    );
  }
  readXmpBatch(
    source: BatchSource,
    options: WorkerBatchOptions = {},
  ): AsyncGenerator<BatchResult<string | undefined>> {
    return this.#batch(source, (image, _edit, controls) => this.readXmp(image, controls), options);
  }
  writeXmpBatch(
    source: BatchSource<XmpEdit>,
    options: WorkerBatchOptions = {},
  ): AsyncGenerator<BatchResult<Uint8Array>> {
    return this.#batch(
      source,
      (image, edit, controls) => this.writeXmp(image, edit, controls),
      options,
    );
  }

  close(): Promise<void> {
    this.#close ??= this.#shutdown();
    return this.#close;
  }
  [Symbol.asyncDispose](): Promise<void> {
    return this.close();
  }

  async #shutdown(): Promise<void> {
    this.#closing = true;
    for (const controller of this.#batches) controller.abort();
    this.#batches.clear();
    for (const task of this.#queue.splice(0)) this.#settle(task, this.#fatal ?? aborted());
    this.#notify();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.#options.closeTimeoutMs);
    try {
      while ([...this.#slots].some((slot) => slot.task)) {
        await withAbort(
          new Promise<void>((resolve) => this.#waiters.add(resolve)),
          controller.signal,
        );
      }
    } catch {
      /* Deadline: terminate CPU-bound or stalled workers below. */
    } finally {
      clearTimeout(timer);
    }
    const slots = [...this.#slots];
    for (const slot of slots) {
      if (slot.task) this.#finish(slot, aborted());
    }
    await Promise.allSettled(slots.map((slot) => slot.worker.terminate()));
    this.#slots.clear();
    this.#notify();
  }
}

export async function createWorkerPool(options: WorkerPoolOptions = {}): Promise<MediaWorkerPool> {
  checkAbort(options.signal);
  const wasm =
    options.wasm instanceof WebAssembly.Module
      ? options.wasm
      : await WebAssembly.compile(
          options.wasm ??
            (await readFile(new URL("../pkg/media_services_wasm_bg.wasm", import.meta.url))),
        );
  checkAbort(options.signal);
  const pool = new MediaWorkerPool(wasm, options);
  await pool.initialise(options.signal);
  return pool;
}
