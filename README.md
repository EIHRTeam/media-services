# media-services

![Rust](https://img.shields.io/badge/Rust-1.96+-000000?logo=rust&logoColor=white)
![WebAssembly](https://img.shields.io/badge/WebAssembly-library-654FF0?logo=webassembly&logoColor=white)
![TypeScript](https://img.shields.io/badge/TypeScript-7-3178C6?logo=typescript&logoColor=white)
![Node.js](https://img.shields.io/badge/Node.js-24-5FA04E?logo=nodedotjs&logoColor=white)
![Chromium](https://img.shields.io/badge/Chromium-130+-4285F4?logo=googlechrome&logoColor=white)

Read and write XMP metadata and add C2PA provenance to images with a TypeScript
API backed by Rust and WebAssembly.

## Requirements

- Node.js 24 or later, or Chromium 130 or later with WebAssembly and `fetch`.
- An ES module environment. CommonJS is not supported.
- For signing, a service implementing the protocol below and a bearer token.

The WebAssembly transport also uses globals available in browser workers. Other
browsers and hosted worker platforms have not been validated by this repository's
test suite.

## Installation

```sh
npm install @eihrteam/mps-worker
```

Before the first npm release, build the repository and install its local tarball
as described under [Publishing](#publishing).

## Quick start

### Read or write metadata

```ts
import { readXmp, writeXmp } from "@eihrteam/mps-worker";

const image = new Uint8Array(await file.arrayBuffer()); // browser File
const packet = await readXmp(image); // undefined when no XMP is present

const updated = await writeXmp(image, {
  creator: ["Example Studio"],
  rights: { "x-default": "Copyright Example Studio. All rights reserved." },
  marked: true,
});

const output = new Blob([Uint8Array.from(updated)], { type: file.type });
```

All image arguments and returned images are `Uint8Array` values. Convert a
browser `File`, `Blob`, or `ArrayBuffer` before calling the API. A Node.js `Buffer`
can be passed directly.

### Write metadata and sign

This Node.js example requires `SIGNER_ENDPOINT` and `SIGNER_TOKEN` in the
process environment and an input JPEG at `input.jpg`.

```ts
import { readFile, writeFile } from "node:fs/promises";
import { connect, type ManifestDefinition } from "@eihrteam/mps-worker";

const manifest: ManifestDefinition = {
  claim_version: 2,
  claim_generator_info: [{ name: "Example Studio", version: "1.0.0" }],
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
};

const provenance = await connect({
  signerEndpoint: process.env.SIGNER_ENDPOINT!,
  token: process.env.SIGNER_TOKEN!,
});

try {
  const { bytes, format, xmp } = await provenance.process(await readFile("input.jpg"), {
    manifest,
    title: "input.jpg",
    xmp: {
      creator: ["Example Studio"],
      rights: { "x-default": "Copyright Example Studio." },
    },
  });
  await writeFile("signed.jpg", bytes);
  console.log(format, xmp);
} finally {
  provenance[Symbol.dispose]();
}
```

Reuse a session for multiple images. Disposal prohibits new calls synchronously
and releases native resources after pending operations finish; repeated calls are harmless. TypeScript
callers may also use `using provenance = await connect(options)`; compile that
syntax for the target runtime.

The image file is processed locally. The service receives the serialized COSE
`Sig_structure`, which contains the signed claim, including claim metadata such
as the asset title. The signing key stays with the service.

## API

| Export                            | Description                                                                                        |
| --------------------------------- | -------------------------------------------------------------------------------------------------- |
| `connect(options)`                | Initializes WebAssembly, fetches signer information once, and returns a `MediaProvenance` session. |
| `session.process(image, options)` | Writes XMP, signs the image, and returns `{ bytes, format, xmp }`.                                 |
| `session[Symbol.dispose]()`       | Releases the session. Further processing with it raises `invalidArgument`.                         |
| `readXmp(image)`                  | Returns the embedded XML packet, or `undefined` if none exists.                                    |
| `writeXmp(image, edit?)`          | Updates XMP without signing and returns the rewritten image.                                       |
| `detectFormat(image)`             | Detects the container MIME type from its header. This is not a full image validation.              |
| `loadWasm(input?)`                | Initializes the shared WebAssembly module, optionally using supplied bytes or a compiled module.   |
| `toProjectTime(value?)`           | Formats a `Date`, epoch milliseconds, or date string in the fixed UTC+8 offset. Defaults to now.   |
| `MediaProvenanceError`            | Error class with a `code` property.                                                                |

Public types include `ConnectOptions`, `ProcessOptions`, `ProcessResult`,
`ManifestDefinition`, `XmpEdit`, `XmpDates`, `XmpValue`, `ErrorCode`, and
`InitInput`. The generated declarations are the authoritative type reference.

### Connection and processing options

`ConnectOptions` requires `signerEndpoint` and `token`. There is no default
endpoint. Its optional `wasm` accepts a `WebAssembly.Module` or `BufferSource`.

`ProcessOptions` requires a nonempty `title` and a `manifest` in c2patool's JSON
manifest-definition format. `xmp` is optional. The pipeline replaces the manifest's
`title` and `format` with the supplied title and detected MIME type.

`ProcessResult.bytes` contains the signed image, `format` is its MIME type, and
`xmp` is the packet written before signing. Successful processing does not itself
establish certificate trust or independently verify the returned signature.

### XMP edits

Edits merge with the existing packet. If none exists, `basePacket` supplies the
starting XML; otherwise the library starts with an empty packet. Omitted fields
are preserved.

| Field          | Value                             | XMP property                                           |
| -------------- | --------------------------------- | ------------------------------------------------------ |
| `creatorTool`  | String                            | `xmp:CreatorTool`                                      |
| `dates`        | `{ create?, modify?, metadata? }` | `xmp:CreateDate`, `xmp:ModifyDate`, `xmp:MetadataDate` |
| `creator`      | String array                      | `dc:creator`, as an ordered sequence                   |
| `source`       | String                            | `dc:source`                                            |
| `rights`       | Language-to-string map            | `dc:rights`                                            |
| `webStatement` | String                            | `xmpRights:WebStatement`                               |
| `usageTerms`   | Language-to-string map            | `xmpRights:UsageTerms`                                 |
| `marked`       | Boolean                           | `xmpRights:Marked`                                     |
| `namespaces`   | Prefix-to-URI map                 | Additional namespace declarations                      |
| `properties`   | Property-to-value map             | Arbitrary `prefix:name` properties                     |

Use `"x-default"` for a language alternative that has no specific language.
General properties are applied after the named fields, so they override a named
field addressing the same property. Signing always sets `dc:format` to the
detected MIME type.

```ts
await writeXmp(image, {
  namespaces: { example: "https://example.org/metadata/" },
  properties: {
    "photoshop:Credit": "Example Studio",
    "dc:subject": { bag: ["landscape", "travel"] },
    "dc:creator": { seq: ["First Author", "Second Author"] },
    "example:Caption": { langAlt: { "x-default": "A landscape", ja: "風景" } },
  },
});
```

A property value is a string or an object containing one of `text`, `seq`, `bag`,
or `langAlt`. Unknown top-level edit fields and date fields are rejected.

`process()` fills missing XMP dates from one clock reading and renders parseable
supplied dates in UTC+8 while preserving their instant. Missing C2PA action times
use the resulting create date; explicitly supplied action times remain unchanged.
Unparseable date strings are forwarded unchanged. `writeXmp()` does not generate
dates automatically. UTC+8 is a package convention, not a C2PA requirement.

### WebAssembly loading

In Node.js, the default loader reads the binary from the installed package. In a
browser, it loads the binary relative to the generated bindings. A bundler or
static host must preserve or serve that asset. Use `application/wasm` as its
response content type.

If the toolchain does not copy the asset automatically, serve the exported
`@eihrteam/mps-worker/wasm-binary` file and load it explicitly before other calls:

```ts
import { loadWasm } from "@eihrteam/mps-worker";

const response = await fetch("/assets/media_services_wasm_bg.wasm");
if (!response.ok) throw new Error(`WASM download failed: ${response.status}`);
await loadWasm(await response.arrayBuffer());
```

Initialization is shared across calls. The first successful initialization fixes
the module used by subsequent sessions. Use bytes or a compiled module for
explicit loading; the current loader does not support every URL/Response form
listed in the generated `InitInput` type.

### Errors

Processing failures use `MediaProvenanceError`. Its documented codes are
`unsupportedFormat`, `malformedXmp`, `containerCorrupt`, `signerUnauthorized`,
`signerRejected`, `networkError`, `invalidSignerInfo`, `malformedManifest`,
`c2paError`, and `invalidArgument`. Unclassified failures can use `unknown`.
WebAssembly download or initialization failures may be ordinary runtime errors.

## Supported formats and limitations

| Format | MIME type    | XMP storage                                         |
| ------ | ------------ | --------------------------------------------------- |
| JPEG   | `image/jpeg` | Adobe XMP APP1 segment, separate from EXIF          |
| PNG    | `image/png`  | Uncompressed `iTXt` chunk                           |
| WebP   | `image/webp` | `XMP ` RIFF chunk with the `VP8X` metadata flag     |
| AVIF   | `image/avif` | `mime` item with content type `application/rdf+xml` |

- AVIF support is limited. Replacing an existing XMP item and layouts using
  non-file-relative construction methods are unsupported. The tests cover one
  AVIF fixture, not all ISO BMFF layout variations.
- Compressed PNG XMP is unsupported. JPEG extended XMP is not assembled.
- Metadata is parsed and serialized, so formatting and whitespace may change.
  This API is not a byte-preserving XML editor.
- Rewriting metadata in a signed image can invalidate its existing C2PA binding.
  Validate the final output with a compatible C2PA verifier.
- The signing path disables automatic post-sign verification. The acceptance
  script requires valid signatures and matching asset hashes. It does not check
  public certificate trust for the generated test CA.

## Signing service protocol

The service is external to this repository. Both requests carry an
`Authorization: Bearer <token>` header.

1. `GET <signerEndpoint>/signer` returns JSON:

   ```json
   {
     "alg": "es256",
     "coseAlg": -7,
     "certsPem": "<PEM certificate chain, leaf first>",
     "reserveSize": 12000
   }
   ```

   Only ES256 is supported. `reserveSize` must be between 1 and 1,048,576 bytes.
   The parsed chain accepts up to 16 certificates of at most 65,536 DER bytes
   each. `coseAlg` is retained in signer information but not independently checked.

2. `POST <signerEndpoint>/sign` accepts the serialized COSE `Sig_structure` as
   `application/octet-stream` and returns a raw 64-byte ES256 `r || s` signature.
   Do not return a DER-encoded signature or a complete COSE object.

HTTP 401 and 403 become `signerUnauthorized`; other unsuccessful responses become
`signerRejected`. Browser use requires the service to support CORS for these
requests, including preflight for the authorization and content-type headers.

## Development

Build prerequisites are Rust 1.96 or later, Node.js 24 or later, pnpm 11.26.0,
and the `wasm32-unknown-unknown` target. The wasm-bindgen CLI must exactly match
its version in `Cargo.lock` (currently 0.2.129). Binaryen's `wasm-opt` is optional.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
pnpm install --frozen-lockfile
pnpm build
pnpm typecheck
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm test
pnpm check:bundle
```

Leave `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` unset during the WASM build; the
build script checks this because the target configuration selects the getrandom
backend. The build enforces a 15 MiB WASM size budget.

`pnpm test` generates a temporary signing identity and runs the Node integration
suite against a local mock service. It needs built `dist/` and `pkg/` artifacts.
The Rust suite covers XMP round trips, container rewrites, signing, and hash
validation after metadata changes.

For the optional independent artifact check:

```sh
cargo install c2patool --version 0.28.1 --locked
cargo test -p provenance --locked
pnpm acceptance
```

The acceptance script reads `target/tmp/signed/`. To exercise a separately
provided signing identity, set `C2PA_TEST_KEY_PEM` and `C2PA_TEST_CHAIN_PEM` to
PEM **file paths** before running the Rust tests. The check validates signatures,
certificate profiles, and asset hashes using explicit settings. It does not
establish that the signing identity is publicly trusted.

### Repository layout

| Path                     | Purpose                                                  |
| ------------------------ | -------------------------------------------------------- |
| `crates/xmp/`            | XMP model, parser, and serializer                        |
| `crates/xmp-image/`      | JPEG, PNG, WebP, and AVIF metadata containers            |
| `crates/provenance/`     | Metadata/signing pipeline and transport abstraction      |
| `crates/wasm/`           | WebAssembly bindings and fetch transport                 |
| `js/`                    | TypeScript API and loader                                |
| `scripts/`               | Build, preset generation, acceptance, and package checks |
| `test/`                  | Node integration and API result tests                    |
| `c2pa/manifest/`, `xmp/` | Organization-specific source templates                   |

`js/presets.ts` is generated from the source templates for repository tests and
local use. It is not part of the published package or public API. Supply your own
manifest and rights metadata rather than relying on these templates.

## Contributing

Open an issue for bugs or proposed API changes, and a pull request for fixes.
Include the runtime, input format, reproduction steps, and expected behavior.
Use a small generated fixture when possible. Run the checks above before
submitting changes and include relevant regression tests for behavior changes.

## Publishing

Release checks use the same build and tests as development. Keep the npm and
Cargo workspace versions aligned. Run all checks, review the package contents,
and confirm third-party license and fixture attribution before the first release.

```sh
npm pack --dry-run --ignore-scripts
npm pack
```

`prepack` builds the package and runs `check:bundle`, which checks the npm file list for
required artifacts, unexpected files, and the existing secret patterns. It is a
packaging sanity check, not a comprehensive audit. Source files are included so
source maps and declaration maps resolve; generated organization presets are
excluded.

Install the resulting tarball in a separate project and confirm imports, types,
WASM loading, and processing. After validation, a maintainer with access to the
npm organization can publish that exact tarball:

```sh
npm publish ./eihrteam-mps-worker-1.0.0.tgz --access public
```

Publishing a tarball does not run the repository's lifecycle checks. Validate it
before publication. Create a matching version tag and GitHub release with a
summary of changes and known limitations.

## License

The project is licensed under [Apache-2.0](LICENSE). Third-party license texts
and attribution are included in `pkg/THIRD_PARTY_LICENSES.txt` in the npm package
and regenerated from the locked dependency tree during the build.
Rights statements embedded in image metadata describe
the image and do not change this project's software license.

## Bounded batches

`readXmpBatch()`, `writeXmpBatch()`, and `session.processBatch()` accept sync or
async iterables and return async iterators. Results arrive in completion order:
`{ index, id?, ok: true, value }` or `{ index, id?, ok: false, error }`. An image
failure does not stop other items. Initialisation and input-iterator failures
terminate the batch.

```ts
import { stat, readFile } from "node:fs/promises";
import { writeXmpBatch } from "@eihrteam/mps-worker";

async function* images(paths: AsyncIterable<string>) {
  for await (const path of paths) {
    const { size } = await stat(path);
    yield {
      id: path,
      image: {
        byteLength: size,
        load: async (signal: AbortSignal) => readFile(path, { signal }),
      },
      options: { creatorTool: "Example Studio" },
    };
  }
}

for await (const item of writeXmpBatch(images(paths))) {
  if (item.ok) await saveImage(item.id!, item.value);
  else console.error(item.id, item.error.code);
}
```

Each item's `image` is a `Uint8Array` or a lazy `{ byteLength, load(signal) }`.
A lazy loader must return exactly its declared byte length and honour
cancellation. An async source must also cooperate with cancellation of its
pending reads; cleanup waits for pending producer and loader operations.
Signing items supply `ProcessOptions` in `options`; writing items supply
`XmpEdit`; reading items omit it.

| Batch option       | Default | Purpose                                                |
| ------------------ | ------- | ------------------------------------------------------ |
| `concurrency`      | 2       | Concurrent loading/processing tasks                    |
| `maxOutstanding`   | 4       | Accepted tasks and completed results awaiting delivery |
| `maxInputBytes`    | 256 MiB | Maximum image size per item                            |
| `maxOutputBytes`   | 320 MiB | Maximum result bytes per item                          |
| `maxInFlightBytes` | 512 MiB | Reserved input bytes during loading and processing     |
| `maxBufferedBytes` | 640 MiB | Output reservations plus completed results             |
| `signal`           | —       | Cancels the entire batch                               |

Budgets must accommodate one maximum-sized item. The scheduler reserves
`maxOutputBytes` before dispatch, then replaces that reservation with the actual
result size. For `process`, result size includes image bytes and UTF-8 XMP; for
`readXmp`, it is UTF-8 XMP. Lowering the per-item output ceiling can permit more
concurrency under the same output budget. Oversized results are rejected after
computation; these limits do not prevent transient native allocations.

Backpressure stops pulling input and scheduling work when consumers are slow.
The source may have one additional pending descriptor awaiting an input-byte
reservation. Use lazy sources for large collections: an array of already-loaded
images still belongs to the caller and cannot be bounded by the scheduler.
Images and results already delivered to the caller, WASM linear memory, allocator
retention, intermediate C2PA allocations, and runtime overhead are outside these
budgets. Process RSS is not a hard limit. WASM linear memory grows and does not
shrink when an image or Session is freed; terminating a Worker releases its realm.

`break` from iteration cancels remaining work and closes the source. Batch
cancellation rejects iteration with `aborted`; per-item cancellation is an item
failure. `connect` and `ProcessOptions` accept `signal` and `requestTimeoutMs`
(default 30,000 ms **per HTTP request**, including reading its response).
`readXmp(image, { signal })` and `writeXmp(image, edit, { signal })` check CPU-phase
boundaries. Synchronous WASM computation cannot be interrupted on the calling
thread. `timeout`, `resourceLimit`, `queueFull`, and `workerError` are additional
stable error codes.

Disposing a Session prohibits new calls immediately and defers native release
until pending calls finish. Use `await session.close()` or `await using` when you
need to wait for that release; repeated disposal/close is harmless.

## Node Worker Threads

The Node-only subpath keeps thread dependencies out of the browser entry point:

```ts
import { createWorkerPool } from "@eihrteam/mps-worker/node";

await using pool = await createWorkerPool({
  workers: 2,
  signing: { signerEndpoint, token }, // omit for metadata-only operations
});

const signed = await pool.process(image, { title: "image.webp", manifest });
const rewritten = await pool.writeXmp(image, { creatorTool: "Example Studio" });
```

The pool exposes `process`, `readXmp`, `writeXmp`, their batch counterparts,
`close`, `Symbol.asyncDispose`, and read-only `stats`. The default worker count is
`min(4, max(1, availableParallelism() - 1))`; each Worker has its own WASM instance
and, when configured, a signing Session. The compiled module is shared; signer
information is fetched once per Worker. One task runs per Worker. Pool batches
default to the worker count, subject to their byte and outstanding-task budgets.

Pool options include `wasm`, `signing`, `workers`, `signal` for startup,
`maxQueuedTasks` (4), `maxInputBytes` (256 MiB), `maxOutputBytes` (320 MiB),
`maxInFlightBytes` (512 MiB), `requestTimeoutMs` (30,000), and `closeTimeoutMs`
(5,000). The pool input-byte budget includes both queued and running tasks.
Direct submissions reject with `queueFull` when full; batches wait for capacity.
Stats report active/queued tasks, reserved input bytes, and last reported total
WASM memory. Main-thread `process.memoryUsage().external/arrayBuffers` does not
include every Worker's heap; use process RSS and pool WASM stats as well.

Inputs are preserved by default. Single operations accept a final
`{ signal?, requestTimeoutMs?, transfer?: boolean }` argument; pool batches accept
`transfer` alongside batch options. `transfer: true` requires a complete,
transferable, exclusively owned ArrayBuffer. It detaches the input as soon as the
pool accepts the task. The caller must ensure there are no other views that need
that backing store. Shared buffers, slices, and Node's pooled Buffers are refused.
Use `Uint8Array.from(buffer)` to obtain exclusive storage. Outputs are transferred
back without another cross-thread image copy.

A Worker crash fails its current task and triggers replacement; signing is never
automatically retried. Closing cancels active batches and their producers as well
as queued work, waits up to the configured
deadline for running work, then terminates remaining Workers. No new calls are
accepted after closing starts. Workers use their own execution arguments rather
than inheriting process/eval/test-runner flags.

## `mps` command line

The npm package contains its WASM binary and a `mps` executable; using the CLI
does not require a Rust toolchain. After this version is published:

```sh
npx @eihrteam/mps-worker xmp read input.webp
npx @eihrteam/mps-worker xmp write input.avif --edit edit.json --output output.avif
npx @eihrteam/mps-worker sign ./images --recursive \
  --manifest manifest.json --output-dir ./signed --workers 4
# Explicit executable selection:
npx --package=@eihrteam/mps-worker mps --help
```

Installing the package also exposes `mps`. Bare `npx mps` refers to a different
npm package name. Before publication, install the locally built tarball and run
`npx --offline @eihrteam/mps-worker --help`.

`sign` requires `--manifest` (a user JSON file). Signing credentials come from
`SIGNER_ENDPOINT` / `SIGNER_TOKEN`, with `--signer-endpoint` and `--token-file`
overrides. `--edit` reads `XmpEdit` JSON and `--base-packet` reads XML used only
when an image lacks XMP. No organization presets, private keys, or tokens are
bundled.

Single-file writes use `--output`; multi-file/directory writes use `--output-dir`
(single files may also use it). Directories are scanned lazily; `--recursive`
enables subdirectories. Symlinks and unsupported extensions are skipped, and the
output directory is excluded from scans. The container is validated from its
bytes when processing. Relative paths are preserved; multiple directory roots
receive separate numbered root subdirectories. Existing destinations are refused
unless `--force` is supplied. Outputs are committed atomically from same-directory
temporary files; interrupted/failed writes remove their temporary files. Atomic
no-overwrite commits require filesystem hard-link support.

`--workers N` opts into the thread pool and defaults to N concurrent tasks.
`--concurrency`, `--max-outstanding`, `--max-input-bytes`, `--max-output-bytes`,
`--max-in-flight-bytes`, `--max-buffered-bytes`, and `--request-timeout-ms` configure
limits. Byte flags accept raw integers or `KiB`/`MiB`/`GiB` suffixes.

Single-file `xmp read` prints XML (empty stdout if absent); batch reads print
JSONL. Writes/signatures report progress to stderr; `--json` produces JSONL
statuses without embedding image bytes. Exit codes are 0 for success, 1 for item
failures, 2 for configuration/initialisation errors, and 130 for SIGINT. Successful
outputs remain when later work fails or is cancelled.

## Performance checks

`pnpm test:pack` installs a local tarball in an isolated directory and checks
bin inference, explicit `mps`, WASM loading and Worker loading. CI runs it after
the integration suite.

```sh
cargo run -p xmp-image --example memory --release --locked -- 32
cargo test -p provenance --test pipeline --locked # also creates plain.webp
pnpm gen:identity
pnpm bench
```

The allocation example asserts that each WebP/AVIF rewrite needs only one
image-sized output allocation plus 128 KiB of metadata overhead for its fixture.
It also limits cumulative rewrite allocations, catching sequential full-image
intermediates. CI runs the 32 MiB case. The benchmark uses fresh Node processes for each
combination of 1/32/128/256 MiB, WebP/AVIF, metadata/signing, and 0/1/2/4 Workers.
After a per-Worker warm-up, it reports startup time, throughput, event-loop latency, RSS high-water marks, sampled main
thread external/ArrayBuffer memory, and WASM memory. Results go to
`target/tmp/benchmark.json`. Override `MPS_BENCH_SIZES` and `MPS_BENCH_ITEMS` for
shorter runs. The large fixtures retain real encoded image payloads and grow
valid ancillary/free boxes, isolating container copying and hashing rather than
codec performance. Benchmarks use a local mock signer and a fixed 512 MiB input
budget, so the largest jobs may use fewer concurrent Workers than configured.

For independent verification of Node/Worker/CLI signatures as well as native
artifacts, use the pinned c2patool 0.28.1:

```sh
MPS_TEST_C2PATOOL=c2patool pnpm test
```

The optional acceptance CI job runs this check. Public certificate trust is
excluded for the generated test CA. Timer durations must be positive integer
milliseconds no greater than 2,147,483,647.

See [performance measurements](docs/performance.md) for the local results and
resource tradeoffs.
