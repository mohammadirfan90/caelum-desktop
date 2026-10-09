// Self-tests for the release-feature guard (CLAUDE.md rule 3, R-CLN-02).
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { findViolations, parseTree, releaseTree } from "../src/release-features.ts";

describe("parseTree / findViolations", () => {
  it("parses package names and features", () => {
    const entries = parseTree(
      "caelum-core v0.1.0 (G:\\repo\\crates\\caelum-core)|test-equipment\n" +
        "serde v1.0.229|default,derive,std\n",
    );
    assert.deepEqual(entries, [
      { name: "caelum-core", features: ["test-equipment"] },
      { name: "serde", features: ["default", "derive", "std"] },
    ]);
  });

  it("flags the test-equipment crate and the feature", () => {
    const problems = findViolations([
      { name: "caelum-test-equipment", features: [] },
      { name: "caelum-domain", features: ["test-equipment"] },
    ]);
    assert.equal(problems.length, 2);
  });

  it("passes a clean graph", () => {
    assert.deepEqual(findViolations([{ name: "caelum-core", features: [] }]), []);
  });
});

describe("real dependency graph (cargo tree)", () => {
  it("the release app graph contains no test equipment", () => {
    const entries = releaseTree();
    assert.ok(entries.some((e) => e.name === "caelum-desktop"), "app package must be in the tree");
    assert.deepEqual(findViolations(entries), []);
  });

  it("enabling the test-equipment feature is caught", () => {
    const problems = findViolations(releaseTree(["test-equipment"]));
    assert.ok(problems.some((p) => p.includes("caelum-test-equipment is in the release dependency graph")));
    assert.ok(problems.some((p) => p.includes("caelum-desktop has the `test-equipment` feature")));
  });
});
