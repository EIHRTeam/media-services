/**
 * Write XMP metadata and C2PA provenance into images.
 *
 * The two steps are ordered deliberately: metadata is written first, then the
 * result is signed, so the metadata ends up inside the signature's asset hash.
 * Changing a character of it afterwards invalidates the manifest.
 *
 * Signing is delegated to a remote service. The image never leaves the caller —
 * only the bytes to be signed cross the network — so the private key stays
 * server-side and the asset stays here.
 *
 * ```ts
 * import { connect } from "@eihrteam/mps-worker";
 *
 * const provenance = await connect({
 *   token,
 *   signerEndpoint: "https://mps.example.org",
 * });
 * const { bytes } = await provenance.process(file, {
 *   manifest,
 *   title: file.name,
 *   xmp: { creatorTool: "Media Provenance Service" },
 * });
 * ```
 */

import { MediaProvenanceError, wrap } from "./errors.js";
import { loadWasm } from "./loader.js";
import { toProcessResult } from "./result.js";
import type { Session } from "../pkg/media_services_wasm.js";

export type { InitInput } from "../pkg/media_services_wasm.js";
export { MediaProvenanceError } from "./errors.js";
export type { ErrorCode } from "./errors.js";

export { loadWasm };

/**
 * A c2patool manifest, which the pipeline accepts unchanged. `title` and
 * `format` are overwritten per asset; everything else is passed through.
 */
export interface ManifestDefinition {
  claim_version?: number;
  title?: string;
  format?: string;
  claim_generator_info?: ReadonlyArray<{
    name: string;
    version?: string;
    /**
     * The C2PA specification version the manifest was produced against, e.g.
     * `"2.4.0"`. Written into the claim itself rather than an assertion.
     *
     * Requires c2pa >= 0.91.2: 0.91.0 serialises the field into the claim's CBOR
     * map as trailing bytes, so every read-back of a manifest carrying it fails
     * with `unexpected trailing data: 18 bytes remaining`.
     */
    specVersion?: string;
  }>;
  assertions?: ReadonlyArray<{
    label: string;
    kind?: string;
    created?: boolean;
    data: unknown;
  }>;
  [key: string]: unknown;
}

/** The three XMP timestamps, as ISO 8601 with a UTC offset. */
export interface XmpDates {
  create?: string;
  modify?: string;
  metadata?: string;
}

/**
 * A value for an arbitrary XMP property.
 *
 * A bare string is a simple value. The tagged forms name the array shape,
 * because `dc:creator` and `dc:subject` are both lists and only one of them is
 * ordered.
 */
export type XmpValue =
  | string
  | { readonly text: string }
  | { readonly seq: readonly string[] }
  | { readonly bag: readonly string[] }
  | { readonly langAlt: Readonly<Record<string, string>> };

/**
 * What to change in the image's XMP. Every field is optional; omitted fields are
 * left as they are, so a second pass merges rather than replaces.
 */
export interface XmpEdit {
  /** A packet to start from when the image carries no XMP of its own. */
  basePacket?: string;
  /** Namespaces for prefixes XMP does not define, as `prefix: uri`. */
  namespaces?: Readonly<Record<string, string>>;
  /**
   * Any property at all, keyed `prefix:name`, for example
   * `{ "photoshop:Credit": "…", "dc:subject": { bag: ["…"] } }`.
   *
   * The named fields below cover one vocabulary; this reaches everything else.
   * Applied last, so it wins over a named field that sets the same property.
   */
  properties?: Readonly<Record<string, XmpValue>>;
  creatorTool?: string;
  dates?: XmpDates;
  /** `dc:creator`, an ordered list. */
  creator?: readonly string[];
  source?: string;
  /** `dc:rights`, keyed by language tag. */
  rights?: Readonly<Record<string, string>>;
  webStatement?: string;
  /** `xmpRights:UsageTerms`, keyed by language tag. */
  usageTerms?: Readonly<Record<string, string>>;
  /** `xmpRights:Marked`. */
  marked?: boolean;
}

/**
 * Where the signing service lives.
 *
 * There is deliberately no default. A baked-in endpoint would send traffic to
 * whichever service shipped with the package — surprising for anyone else using
 * it, and unwanted load for whoever runs that service.
 */
export interface ConnectOptions {
  /**
   * The signing service's base URL, for example `https://mps.example.org`.
   * Required: see the note on this interface.
   */
  signerEndpoint: string;
  /**
   * The bearer credential for the signing service.
   *
   * In a browser this is visible to whoever is using the page, so issue
   * short-lived, narrowly-scoped tokens rather than putting a long-lived one
   * here.
   */
  token: string;
  /**
   * A pre-compiled module or its bytes. Only needed when the module cannot be
   * located relative to this file, or when loading it should be done by the
   * caller.
   */
  wasm?: WebAssembly.Module | BufferSource;
}

export interface ProcessOptions {
  /** The manifest to sign with. `js/presets.ts` holds this repo's own. */
  manifest: ManifestDefinition;
  /** A name for the asset, used as the manifest's `title`. */
  title: string;
  xmp?: XmpEdit;
}

export interface ProcessResult {
  /** The rewritten, signed image. */
  bytes: Uint8Array;
  /** The container's MIME type, detected rather than assumed. */
  format: string;
  /** The XMP packet as written, so a caller can show what was stamped. */
  xmp: string;
}

/**
 * The offset every timestamp this package writes is rendered in.
 *
 * Fixed rather than read from the machine. A Cloudflare Worker reports a local
 * offset of zero, so the same call would otherwise stamp a packet differently
 * depending on whether it ran there, on a laptop in Asia/Shanghai, or on a CI
 * runner — for an asset whose whole point is to say when it was stamped.
 */
const PROJECT_OFFSET_MINUTES = 8 * 60;

function pad2(value: number): string {
  return String(value).padStart(2, "0");
}

/**
 * Renders an instant as ISO 8601 in this package's fixed UTC+8 offset.
 * UTC is also a valid representation; this helper preserves the project's
 * timestamp convention across hosts.
 */
export function toProjectTime(value: Date | number | string = new Date()): string {
  const instant = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(instant.getTime())) {
    throw new MediaProvenanceError("invalidArgument", `not a timestamp: ${String(value)}`);
  }

  // Shift the instant and then read its UTC fields: the offset has been applied
  // already, so those fields are the local ones for the target zone. Being a
  // fixed offset, there is no daylight-saving transition to get wrong.
  const shifted = new Date(instant.getTime() + PROJECT_OFFSET_MINUTES * 60_000);
  const sign = PROJECT_OFFSET_MINUTES >= 0 ? "+" : "-";
  const absolute = Math.abs(PROJECT_OFFSET_MINUTES);
  return (
    `${shifted.getUTCFullYear()}-${pad2(shifted.getUTCMonth() + 1)}-${pad2(shifted.getUTCDate())}` +
    `T${pad2(shifted.getUTCHours())}:${pad2(shifted.getUTCMinutes())}:${pad2(shifted.getUTCSeconds())}` +
    `${sign}${pad2(Math.floor(absolute / 60))}:${pad2(absolute % 60)}`
  );
}

/**
 * Re-renders a caller's timestamp in UTC+8. The instant is unchanged; only its
 * representation moves, which is the point — one timezone across every packet.
 *
 * A value that does not parse is passed through untouched rather than rejected:
 * this package is not the validator, and mangling a string it does not
 * understand would be worse than forwarding it.
 */
function normaliseStamp(value: string): string {
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? value : toProjectTime(parsed);
}

/** The three XMP dates, all set to one instant in the project's timezone. */
function datesFor(instant: string, given?: XmpDates): XmpDates {
  const dates: XmpDates = { create: instant, modify: instant, metadata: instant };
  if (given === undefined) return dates;
  for (const field of ["create", "modify", "metadata"] as const) {
    const value = given[field];
    if (value !== undefined) dates[field] = normaliseStamp(value);
  }
  return dates;
}

/** A configured session, reused across images. */
export class MediaProvenance {
  readonly #session: Session;
  readonly endpoint: string;
  #released = false;

  /** @internal — use {@link connect}. */
  constructor(session: Session, endpoint: string) {
    this.#session = session;
    this.endpoint = endpoint;
  }

  /**
   * Writes XMP, then signs the result.
   *
   * The order is not incidental: signing covers the whole asset, so metadata
   * written here is covered by that hash. Writing it afterwards would leave it
   * with no provenance attached.
   */
  async process(image: Uint8Array, options: ProcessOptions): Promise<ProcessResult> {
    // Checked here rather than left to the module. A released session has a null
    // pointer behind it, and the resulting call aborts the module instead of
    // rejecting — the caller would wait on a promise that never settles.
    if (this.#released) {
      throw new MediaProvenanceError(
        "invalidArgument",
        "this session has been released; connect again to sign another image",
      );
    }
    if (!options?.manifest) {
      throw new MediaProvenanceError("invalidArgument", "a manifest is required");
    }
    if (!options.title) {
      throw new MediaProvenanceError("invalidArgument", "a title is required");
    }
    try {
      // Every timestamp is the moment this runs, rendered in UTC+8. One clock
      // reading feeds both the XMP dates and the manifest's action times, so the
      // two cannot disagree about when the asset was stamped. A caller who pins
      // a value keeps that instant, re-rendered in the same zone.
      const xmp: XmpEdit = {
        ...options.xmp,
        dates: datesFor(toProjectTime(), options.xmp?.dates),
      };

      return toProcessResult(
        await this.#session.process(image, xmp, JSON.stringify(options.manifest), options.title),
      );
    } catch (error) {
      throw wrap(error);
    }
  }

  /**
   * Releases the session's WebAssembly-side resources.
   *
   * A session owns module heap that nothing reclaims on its own, so connecting
   * per request leaks until the page goes away. `using` calls this at the end of
   * the enclosing block:
   *
   * ```ts
   * using provenance = await connect({ token, signerEndpoint });
   * ```
   *
   * Plain `using` rather than `await using`: the release is synchronous, and the
   * async form would additionally require `Symbol.asyncDispose` to exist at
   * runtime for no gain.
   *
   * A second call is ignored rather than forwarded. The generated `free()`
   * zeroes the pointer and then hands it to the module anyway, so disposing
   * twice would pass a null pointer back into wasm — and disposing twice is an
   * ordinary thing to write: `using` alongside an explicit call, or two
   * overlapping scopes.
   */
  [Symbol.dispose](): void {
    if (this.#released) return;
    this.#released = true;
    this.#session.free();
  }
}

/**
 * The XMP packet an image carries, or `undefined` if it has none.
 *
 * Reading metadata needs no credential and no signing service, so this is a
 * plain function rather than a method on a connected session.
 */
export async function readXmp(image: Uint8Array): Promise<string | undefined> {
  await loadWasm();
  const { readXmp: native } = await import("../pkg/media_services_wasm.js");
  try {
    return native(image);
  } catch (error) {
    throw wrap(error);
  }
}

/** Writes XMP without signing. Returns the rewritten image. */
export async function writeXmp(image: Uint8Array, edit: XmpEdit = {}): Promise<Uint8Array> {
  await loadWasm();
  const { writeXmp: native } = await import("../pkg/media_services_wasm.js");
  try {
    return native(image, edit);
  } catch (error) {
    throw wrap(error);
  }
}

/**
 * Loads the module and connects to the signing service.
 *
 * This performs a `GET /signer` to fetch the certificate chain, so it is worth
 * calling once and keeping the result rather than per image.
 */
export async function connect(options: ConnectOptions): Promise<MediaProvenance> {
  if (!options?.token) {
    throw new MediaProvenanceError("invalidArgument", "a token is required");
  }
  const endpoint = (options.signerEndpoint ?? "").trim().replace(/\/+$/, "");
  if (!endpoint) {
    throw new MediaProvenanceError(
      "invalidArgument",
      "signerEndpoint is required; there is no default signing service",
    );
  }
  // Checked here rather than letting `fetch` fail later: a mistyped scheme or a
  // missing host surfaces as a confusing network error otherwise.
  try {
    new URL(endpoint);
  } catch {
    throw new MediaProvenanceError(
      "invalidArgument",
      `signerEndpoint is not a valid URL: ${endpoint}`,
    );
  }

  await loadWasm(options.wasm);
  const { Session } = await import("../pkg/media_services_wasm.js");

  try {
    const session = await Session.create(endpoint, options.token);
    return new MediaProvenance(session, endpoint);
  } catch (error) {
    throw wrap(error);
  }
}

/** The container MIME type of `image`, or an error if it is not supported. */
export async function detectFormat(image: Uint8Array): Promise<string> {
  await loadWasm();
  const { detectFormat: native } = await import("../pkg/media_services_wasm.js");
  try {
    return native(image);
  } catch (error) {
    throw wrap(error);
  }
}
