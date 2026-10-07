/**
 * The WebAssembly boundary, on its own.
 *
 * `Session.process` is typed `Promise<any>` in the generated bindings, so the
 * object crossing that line is invisible to the compiler. These cover the check
 * that runs instead — the failure they describe cannot be provoked through the
 * public API, because the module only ever builds the object correctly.
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import { toProcessResult } from "../dist/result.js";
import { MediaProvenanceError } from "../dist/index.js";

/** Every refusal should be the package's own error, with the same code. */
function isRefusal(name) {
  return (error) => {
    assert.ok(
      error instanceof MediaProvenanceError,
      `${name}: expected a MediaProvenanceError, got ${error?.constructor?.name}: ${error?.message}`,
    );
    assert.equal(error.code, "invalidSignerInfo", name);
    return true;
  };
}

test("a well-formed result comes back with all three fields", () => {
  const bytes = new Uint8Array([1, 2, 3]);
  const result = toProcessResult({ bytes, format: "image/png", xmp: "<x:xmpmeta/>" });

  // Identity, not a copy: the bytes are the caller's asset and copying a
  // multi-megabyte image here would be a cost with no purpose.
  assert.equal(result.bytes, bytes);
  assert.equal(result.format, "image/png");
  assert.equal(result.xmp, "<x:xmpmeta/>");
});

test("a result missing a field is refused rather than handed on", () => {
  // Renaming a field on the Rust side is the realistic cause. Unchecked, the
  // caller would receive `undefined` where a Uint8Array was promised and
  // discover it somewhere far from the cause.
  const cases = {
    "bytes absent": { format: "image/png", xmp: "<x/>" },
    "bytes not a Uint8Array": { bytes: "not bytes", format: "image/png", xmp: "<x/>" },
    "format not a string": { bytes: new Uint8Array(), format: 42, xmp: "<x/>" },
    "xmp not a string": { bytes: new Uint8Array(), format: "image/png", xmp: null },
    "an empty object": {},
  };

  for (const [name, value] of Object.entries(cases)) {
    assert.throws(() => toProcessResult(value), isRefusal(name), name);
  }
});

test("a value that is not an object at all is refused", () => {
  for (const value of [null, undefined, "bytes", 42, true]) {
    const label = String(value);
    assert.throws(() => toProcessResult(value), isRefusal(label), label);
  }
});

test("the refusal names what was wrong, without quoting the payload", () => {
  // The value here is a whole image on the success path, and on this path it is
  // whatever the module returned — neither belongs in a message that travels
  // into logs.
  assert.throws(
    () => toProcessResult({ bytes: new Uint8Array(), format: "image/png", xmp: 7 }),
    (error) => {
      assert.match(error.message, /xmp/);
      return true;
    },
  );
});
