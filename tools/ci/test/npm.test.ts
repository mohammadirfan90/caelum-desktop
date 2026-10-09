// Self-tests for the npm licence guard and the UI SBOM (CR-003 §4, docs/09_VV_PLAN.md §4).
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  ALLOWED_LICENSES,
  checkNpmLicenses,
  cycloneDx,
  licenseAllowed,
  parsePnpmLicenses,
  purl,
  shippedUiPackages,
} from "../src/npm.ts";

describe("licence expressions", () => {
  it("accepts allowed single licences and OR alternatives", () => {
    assert.ok(licenseAllowed("MIT", ALLOWED_LICENSES));
    assert.ok(licenseAllowed("Apache-2.0 OR MIT", ALLOWED_LICENSES));
    assert.ok(licenseAllowed("(MIT OR GPL-3.0-only)", ALLOWED_LICENSES));
  });

  it("rejects copyleft and requires every AND part", () => {
    assert.ok(!licenseAllowed("GPL-3.0-only", ALLOWED_LICENSES));
    assert.ok(!licenseAllowed("LGPL-2.1-or-later", ALLOWED_LICENSES));
    assert.ok(!licenseAllowed("MIT AND GPL-2.0-only", ALLOWED_LICENSES));
    assert.ok(licenseAllowed("BSD-3-Clause AND MIT", ALLOWED_LICENSES));
  });
});

describe("checkNpmLicenses", () => {
  it("allows OFL-1.1 only for bundled font packages", () => {
    assert.deepEqual(checkNpmLicenses([{ name: "@fontsource/outfit", version: "5.3.0", license: "OFL-1.1" }]), []);
    assert.equal(checkNpmLicenses([{ name: "some-lib", version: "1.0.0", license: "OFL-1.1" }]).length, 1);
  });

  it("flags a GPL package", () => {
    const problems = checkNpmLicenses([{ name: "gpl-lib", version: "2.0.0", license: "GPL-3.0-only" }]);
    assert.deepEqual(problems, ['gpl-lib@2.0.0: licence "GPL-3.0-only" is not allowed']);
  });
});

describe("pnpm output and SBOM", () => {
  it("parses pnpm licenses JSON", () => {
    const json = JSON.stringify({ MIT: [{ name: "react", versions: ["19.3.0"] }] });
    assert.deepEqual(parsePnpmLicenses(json), [{ name: "react", version: "19.3.0", license: "MIT" }]);
  });

  it("builds a CycloneDX 1.6 document with package URLs and licences", () => {
    const bom = cycloneDx({ name: "@caelum/ui", version: "0.1.0" }, [
      { name: "@tauri-apps/api", version: "2.12.2", license: "Apache-2.0 OR MIT" },
    ]);
    assert.equal(bom.bomFormat, "CycloneDX");
    assert.equal(bom.specVersion, "1.6");
    assert.match(bom.serialNumber, /^urn:uuid:[0-9a-f-]{36}$/);
    assert.deepEqual(bom.components[0], {
      type: "library",
      "bom-ref": "pkg:npm/%40tauri-apps/api@2.12.2",
      name: "@tauri-apps/api",
      version: "2.12.2",
      purl: "pkg:npm/%40tauri-apps/api@2.12.2",
      licenses: [{ expression: "Apache-2.0 OR MIT" }],
    });
    assert.equal(purl("react", "19.3.0"), "pkg:npm/react@19.3.0");
  });
});

describe("repository", () => {
  it("the shipped UI packages all have allowed licences", () => {
    const packages = shippedUiPackages();
    assert.ok(packages.some((p) => p.name === "react"), "react must be listed as shipped");
    assert.ok(!packages.some((p) => p.name === "vite"), "dev dependencies must not be listed");
    assert.deepEqual(checkNpmLicenses(packages), []);
  });
});
