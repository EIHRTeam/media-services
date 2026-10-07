# media-services

![Rust](https://img.shields.io/badge/Rust-1.96+-000000?logo=rust&logoColor=white)
![WebAssembly](https://img.shields.io/badge/WebAssembly-library-654FF0?logo=webassembly&logoColor=white)
![TypeScript](https://img.shields.io/badge/TypeScript-7-3178C6?logo=typescript&logoColor=white)
![Node.js](https://img.shields.io/badge/Node.js-24-5FA04E?logo=nodedotjs&logoColor=white)
![Chromium](https://img.shields.io/badge/Chromium-130+-4285F4?logo=googlechrome&logoColor=white)

Read and write XMP metadata and add C2PA provenance to images with a TypeScript
API backed by Rust and WebAssembly.

The npm package is `@eihrteam/mps-worker`. It writes metadata before signing so
that the resulting XMP is covered by the C2PA asset hash. Signing uses a
caller-provided remote service; metadata-only operations work locally.

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
  assertions: [{
    label: "c2pa.actions.v2",
    data: { actions: [{ action: "c2pa.created" }] },
  }],
};

const provenance = await connect({
  signerEndpoint: process.env.SIGNER_ENDPOINT!,
  token: process.env.SIGNER_TOKEN!,
});

try {
  const { bytes, format, xmp } = await provenance.process(
    await readFile("input.jpg"),
    {
      manifest,
      title: "input.jpg",
      xmp: {
        creator: ["Example Studio"],
        rights: { "x-default": "Copyright Example Studio." },
      },
    },
  );
  await writeFile("signed.jpg", bytes);
  console.log(format, xmp);
} finally {
  provenance[Symbol.dispose]();
}
```

Reuse a session for multiple images, and release it after all pending operations
finish. Disposal is synchronous and repeated calls are harmless. TypeScript
callers may also use `using provenance = await connect(options)`; compile that
syntax for the target runtime.

The image file is processed locally. The service receives the serialized COSE
`Sig_structure`, which contains the signed claim, including claim metadata such
as the asset title. The signing key stays with the service.

## API

| Export | Description |
| --- | --- |
| `connect(options)` | Initializes WebAssembly, fetches signer information once, and returns a `MediaProvenance` session. |
| `session.process(image, options)` | Writes XMP, signs the image, and returns `{ bytes, format, xmp }`. |
| `session[Symbol.dispose]()` | Releases the session. Further processing with it raises `invalidArgument`. |
| `readXmp(image)` | Returns the embedded XML packet, or `undefined` if none exists. |
| `writeXmp(image, edit?)` | Updates XMP without signing and returns the rewritten image. |
| `detectFormat(image)` | Detects the container MIME type from its header. This is not a full image validation. |
| `loadWasm(input?)` | Initializes the shared WebAssembly module, optionally using supplied bytes or a compiled module. |
| `toProjectTime(value?)` | Formats a `Date`, epoch milliseconds, or date string in the fixed UTC+8 offset. Defaults to now. |
| `MediaProvenanceError` | Error class with a `code` property. |

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

| Field | Value | XMP property |
| --- | --- | --- |
| `creatorTool` | String | `xmp:CreatorTool` |
| `dates` | `{ create?, modify?, metadata? }` | `xmp:CreateDate`, `xmp:ModifyDate`, `xmp:MetadataDate` |
| `creator` | String array | `dc:creator`, as an ordered sequence |
| `source` | String | `dc:source` |
| `rights` | Language-to-string map | `dc:rights` |
| `webStatement` | String | `xmpRights:WebStatement` |
| `usageTerms` | Language-to-string map | `xmpRights:UsageTerms` |
| `marked` | Boolean | `xmpRights:Marked` |
| `namespaces` | Prefix-to-URI map | Additional namespace declarations |
| `properties` | Property-to-value map | Arbitrary `prefix:name` properties |

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

| Format | MIME type | XMP storage |
| --- | --- | --- |
| JPEG | `image/jpeg` | Adobe XMP APP1 segment, separate from EXIF |
| PNG | `image/png` | Uncompressed `iTXt` chunk |
| WebP | `image/webp` | `XMP ` RIFF chunk with the `VP8X` metadata flag |
| AVIF | `image/avif` | `mime` item with content type `application/rdf+xml` |

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
backend. The build enforces a 9 MiB WASM size budget.

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

| Path | Purpose |
| --- | --- |
| `crates/xmp/` | XMP model, parser, and serializer |
| `crates/xmp-image/` | JPEG, PNG, WebP, and AVIF metadata containers |
| `crates/provenance/` | Metadata/signing pipeline and transport abstraction |
| `crates/wasm/` | WebAssembly bindings and fetch transport |
| `js/` | TypeScript API and loader |
| `scripts/` | Build, preset generation, acceptance, and package checks |
| `test/` | Node integration and API result tests |
| `c2pa/manifest/`, `xmp/` | Organization-specific source templates |

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
