/**
 * The version stamped into the generated presets.
 *
 * These manifests name the software that produced the claim, and the version
 * they named was a hand-copied literal: a second copy of the package version
 * that no release updated, so every claim built from the repository's own
 * presets described a version of the service that had long since moved on.
 *
 * The templates now carry no version and the generator writes the current one
 * in. These tests hold both halves of that: that the stamp lands wherever the
 * manifest names this software, and that no template has grown a literal back.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { stampSoftwareVersion } from "../scripts/gen-presets.mjs";
import { manifests } from "../dist/presets.js";

const root = fileURLToPath(new URL("..", import.meta.url));
const TEMPLATES = readdirSync(join(root, "c2pa/manifest")).filter((name) => name.endsWith(".json"));

/** The assertion carrying the actions, whichever position it sits in. */
function actionsOf(manifest) {
  const assertion = manifest.assertions.find((entry) => entry.label === "c2pa.actions.v2");
  assert.ok(assertion, "the manifest has no c2pa.actions.v2 assertion");
  return assertion.data.actions;
}

/** A version that is plainly not the package's, so a pass cannot be luck. */
const SENTINEL = "9.9.9";

test("no template names a version of its own", () => {
  for (const name of TEMPLATES) {
    const manifest = JSON.parse(readFileSync(join(root, "c2pa/manifest", name), "utf8"));

    for (const info of manifest.claim_generator_info) {
      assert.ok(!("version" in info), `${name}: the claim generator carries a version literal`);
    }
    for (const action of actionsOf(manifest)) {
      assert.ok(
        !("version" in action.softwareAgent),
        `${name}: the ${action.action} software agent carries a version literal`,
      );
    }
  }
});

test("the generator stamps the version on the claim generator and every action", () => {
  for (const name of TEMPLATES) {
    const manifest = JSON.parse(readFileSync(join(root, "c2pa/manifest", name), "utf8"));
    const stamped = stampSoftwareVersion(manifest, SENTINEL);

    for (const info of stamped.claim_generator_info) {
      assert.equal(info.version, SENTINEL, name);
    }
    for (const action of actionsOf(stamped)) {
      assert.equal(action.softwareAgent.version, SENTINEL, `${name}: ${action.action}`);
    }
  }
});

test("a version that belongs to something else is left alone", () => {
  // C2PA manifests carry versions that are not the generator's — a spec
  // version here, a schema version in an assertion there. A stamp that reached
  // every `version` key it could find would rewrite those too.
  const manifest = {
    claim_generator_info: [{ name: "Test Service", specVersion: "2.4.0" }],
    assertions: [
      {
        label: "c2pa.actions.v2",
        data: {
          schemaVersion: "7",
          actions: [{ action: "c2pa.created", softwareAgent: { name: "Test Service" } }],
        },
      },
    ],
  };
  const original = structuredClone(manifest);

  const stamped = stampSoftwareVersion(manifest, SENTINEL);

  assert.equal(stamped.claim_generator_info[0].specVersion, "2.4.0");
  assert.equal(stamped.assertions[0].data.schemaVersion, "7");
  assert.equal(stamped.assertions[0].data.actions[0].softwareAgent.version, SENTINEL);
  // A fresh document, so a caller's template is not rewritten in place.
  assert.deepEqual(manifest, original);
});

test("the presets this build produced carry the package's own version", () => {
  const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));

  for (const [name, manifest] of Object.entries(manifests)) {
    for (const info of manifest.claim_generator_info) {
      assert.equal(info.version, version, name);
    }
    for (const action of actionsOf(manifest)) {
      assert.equal(action.softwareAgent.version, version, `${name}: ${action.action}`);
    }
  }
});
