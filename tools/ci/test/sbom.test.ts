// Release SBOMs (docs/09_VV_PLAN.md §4): generated for real and checked for content.
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { REPO_ROOT } from "../src/repo.ts";
import { writeSboms } from "../src/sbom.ts";

interface Bom {
  readonly bomFormat: string;
  readonly specVersion: string;
  readonly metadata: { readonly component: { readonly name: string; readonly version: string } };
  readonly components: readonly { readonly name: string; readonly licenses?: readonly unknown[] }[];
}

const load = (path: string) => JSON.parse(readFileSync(path, "utf8")) as Bom;

describe("release SBOMs", () => {
  const { rust, ui } = writeSboms();

  it("the Rust SBOM describes caelum-desktop 0.1.0 with licences for every component", () => {
    const bom = load(rust);
    assert.equal(bom.bomFormat, "CycloneDX");
    assert.equal(bom.metadata.component.name, "caelum-desktop");
    assert.equal(bom.metadata.component.version, "0.1.0");
    assert.ok(bom.components.some((c) => c.name === "tauri"));
    assert.deepEqual(bom.components.filter((c) => (c.licenses ?? []).length === 0).map((c) => c.name), []);
  });

  it("the Rust SBOM contains no test equipment (CLAUDE.md rule 3)", () => {
    const names = load(rust).components.map((c) => c.name);
    assert.ok(!names.includes("caelum-test-equipment"));
  });

  it("the UI SBOM lists exactly the shipped npm packages", () => {
    const names = load(ui).components.map((c) => c.name);
    assert.ok(names.includes("react") && names.includes("@tauri-apps/api") && names.includes("@fontsource/outfit"));
    assert.ok(!names.includes("vite") && !names.includes("vitest"), "dev dependencies must not be listed");
  });

  it("leaves no stray SBOM files in the source tree", () => {
    const strays = ["src-tauri", ...readdirSync(join(REPO_ROOT, "crates")).map((c) => join("crates", c))]
      .map((dir) => join(REPO_ROOT, dir, "caelum-desktop.cdx.json"))
      .filter((path) => existsSync(path));
    assert.deepEqual(strays, []);
  });
});
