// trace: ignore-ids (fixtures below contain example IDs; this file cites none)
// Self-tests for the traceability generator (docs/09_VV_PLAN.md §3, CLAUDE.md rule 9).
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { REPO_ROOT } from "../src/repo.ts";
import { type Catalog, type CatalogEntry, readCatalog } from "../src/trace/catalog.ts";
import { extractIds, idsFromName } from "../src/trace/ids.ts";
import { parseJunit } from "../src/trace/junit.ts";
import { scanRepository, scanRust, scanTs, type TestRef, titlePattern } from "../src/trace/scan.ts";
import { buildTrace, casesFor, enforcement, type Results, toCsv } from "../src/trace/trace.ts";

describe("ID extraction", () => {
  it("expands lists and ranges as the docs write them", () => {
    assert.deepEqual(extractIds("F-ACQ-01/02/04"), ["F-ACQ-01", "F-ACQ-02", "F-ACQ-04"]);
    assert.equal(extractIds("SR-001..006").length, 6);
    assert.equal(extractIds("SR-001..SR-019").length, 19);
    assert.deepEqual(extractIds("R-INT-05..07"), ["R-INT-05", "R-INT-06", "R-INT-07"]);
    assert.equal(extractIds("F-ACQ-01…05").length, 5); // 02's need table uses an ellipsis
    assert.deepEqual(extractIds("F-ML-02 and N-01"), ["F-ML-02", "N-01"]);
  });

  it("ignores things that are not requirement IDs", () => {
    assert.deepEqual(extractIds("CR-002 D5, H-07, OI-1, M0, ICD-VEH, RUSTSEC-2024-0429"), []);
  });

  it("reads snake-case IDs in Rust test names", () => {
    assert.deepEqual(idsFromName("sr_012_clock_fault_raises_alarm"), ["SR-012"]);
    assert.deepEqual(idsFromName("r_sec_04_no_command_path"), ["R-SEC-04"]);
    assert.deepEqual(idsFromName("f_alm_03_priorities"), ["F-ALM-03"]);
    assert.deepEqual(idsFromName("plain_test_name"), []);
  });
});

describe("catalog from the docs", () => {
  const catalog = readCatalog(REPO_ROOT);

  it("reads features with priority and milestone", () => {
    assert.deepEqual(
      { priority: catalog.entries.get("F-SYS-06")?.priority, milestones: catalog.entries.get("F-SYS-06")?.milestones },
      { priority: "M", milestones: ["M0"] },
    );
    assert.equal(catalog.entries.get("F-SYS-13")?.priority, "S");
    assert.deepEqual(catalog.entries.get("F-SYS-13")?.milestones, ["M11"]);
  });

  it("reads requirements, safety requirements and needs", () => {
    assert.equal(catalog.entries.get("R-SEC-07")?.kind, "requirement");
    assert.equal(catalog.entries.get("SR-019")?.kind, "safety");
    assert.ok(catalog.entries.get("N-02")?.features.includes("F-ACQ-05"));
    assert.deepEqual(catalog.entries.get("SR-003")?.milestones, ["M3"]);
  });

  it("reads milestone status from the build plan", () => {
    assert.ok(catalog.milestoneStatus.has("M0"));
    assert.equal(catalog.milestoneStatus.get("M13"), "not started");
  });
});

describe("source scanning", () => {
  it("finds Rust tests with IDs from name, comment above and file header", () => {
    const text = [
      "//! Module checks (R-SEC-04).",
      "#[cfg(test)]",
      "mod tests {",
      "    /// SR-011: vehicle stays read-only.",
      "    #[test]",
      "    fn sr_012_and_more() {}",
      "    #[test]",
      "    #[ignore]",
      "    fn header_only() {}",
      "}",
    ].join("\n");
    const tests = scanRust("crates/x/src/lib.rs", "x", text);
    assert.deepEqual(tests.map((t) => [t.name, t.ids]), [
      ["sr_012_and_more", ["R-SEC-04", "SR-011", "SR-012"]],
      ["header_only", ["R-SEC-04"]],
    ]);
  });

  it("records the operating system of tests gated with cfg", () => {
    const text = [
      "#[cfg(windows)]",
      "#[test]",
      "fn only_windows() {}",
      "#[test]",
      '#[cfg(target_os = "macos")]',
      "fn only_macos() {}",
      "#[test]",
      "fn everywhere() {}",
    ].join("\n");
    const tests = scanRust("crates/x/tests/os.rs", "x", text);
    assert.deepEqual(tests.map((t) => [t.name, t.platform]), [
      ["only_windows", "windows"],
      ["only_macos", "macos"],
      ["everywhere", undefined],
    ]);
  });

  it("finds TS tests incl. it.each and template titles", () => {
    const text = [
      "// Contract (F-SYS-10).",
      'import { it } from "vitest";',
      "// R-ALM-02",
      'it("plain title", () => {});',
      "it.each(pairs.map((p) => [p.fg, p] as const))(",
      '  "%s meets its threshold",',
      "  () => {});",
      "it(`catches ${rule.id} quickly`, () => {});",
    ].join("\n");
    const tests = scanTs("ui/tests/x.test.ts", "ui", "vitest", text);
    assert.deepEqual(tests.map((t) => t.name), ["plain title", "%s meets its threshold", "catches ${rule.id} quickly"]);
    assert.deepEqual(tests[0]?.ids, ["F-SYS-10", "R-ALM-02"]);
    assert.deepEqual(tests[1]?.ids, ["F-SYS-10"]);
  });

  it("turns titles into patterns that match the reported names", () => {
    assert.ok(titlePattern("%s meets its threshold").test("light theme > ink on canvas (text) meets its threshold"));
    assert.ok(titlePattern("catches ${rule.id} in caelum-devices").test("catches io-write in caelum-devices"));
    assert.ok(titlePattern("never uses --warning (fill only, CR-002 D5)").test("pair contract > never uses --warning (fill only, CR-002 D5)"));
    assert.ok(!titlePattern("plain title").test("another plain title"));
  });

  it("skips files marked trace: ignore-ids (fixtures are not tests or citations)", () => {
    const { tests, modules } = scanRepository(REPO_ROOT);
    assert.ok(!modules.some((m) => m.file.startsWith("tools/ci/src/trace/")));
    assert.ok(!tests.some((t) => t.file === "tools/ci/test/trace.test.ts"));
  });
});

describe("JUnit reports", () => {
  it("reads passed, failed and skipped cases and decodes entities", () => {
    const xml = [
      '<testsuite name="s"><testcase name="a &gt; b" classname="tests/x.test.ts" time="0.1"/>',
      '<testcase classname="crate" name="m::failing"><failure message="boom"/></testcase>',
      '<testcase classname="crate" name="m::skipped"><skipped/></testcase></testsuite>',
    ].join("");
    assert.deepEqual(parseJunit(xml), [
      { classname: "tests/x.test.ts", name: "a > b", outcome: "passed" },
      { classname: "crate", name: "m::failing", outcome: "failed" },
      { classname: "crate", name: "m::skipped", outcome: "skipped" },
    ]);
  });

  it("matches Rust unit and integration tests to their binaries", () => {
    const results: Results = {
      nextest: [
        { classname: "caelum-desktop", name: "commands::tests::app_info", outcome: "passed" },
        { classname: "caelum-core::architecture", name: "rule_a", outcome: "passed" },
        { classname: "caelum-core::other", name: "rule_a", outcome: "failed" },
      ],
      vitest: [],
      node: [],
    };
    const unit: TestRef = { runner: "nextest", file: "src-tauri/src/commands.rs", unit: "caelum-desktop", name: "app_info", ids: [] };
    const integration: TestRef = { runner: "nextest", file: "crates/caelum-core/tests/architecture.rs", unit: "caelum-core", name: "rule_a", ids: [] };
    assert.equal(casesFor(unit, results).length, 1);
    assert.deepEqual(casesFor(integration, results).map((c) => c.outcome), ["passed"]);
  });
});

function catalogOf(entries: CatalogEntry[], status: Record<string, string>): Catalog {
  return { entries: new Map(entries.map((e) => [e.id, e])), milestoneStatus: new Map(Object.entries(status)) };
}

const feature = (id: string, priority: string, milestones: string[]): CatalogEntry => ({
  id,
  kind: "feature",
  priority,
  features: [],
  milestones,
});

describe("trace rows and the gate", () => {
  const catalog = catalogOf(
    [feature("F-AAA-01", "M", ["M1"]), feature("F-AAA-02", "M", ["M1"]), feature("F-AAA-03", "S", ["M1"]), feature("F-AAA-04", "M", ["M2"])],
    { M1: "complete", M2: "in progress" },
  );
  const test = (name: string, ids: string[]): TestRef => ({ runner: "node", file: "tools/ci/test/x.test.ts", unit: "tools/ci", name, ids });
  const results: Results = {
    nextest: [],
    vitest: [],
    node: [
      { classname: "tools/ci/test/x.test.ts", name: "passes", outcome: "passed" },
      { classname: "tools/ci/test/x.test.ts", name: "fails", outcome: "failed" },
    ],
  };

  it("enforces only M-priority features of complete milestones", () => {
    const trace = buildTrace(catalog, [test("passes", ["F-AAA-01"]), test("fails", ["F-AAA-02"])], [], results);
    assert.deepEqual(
      trace.rows.map((r) => [r.id, r.result]),
      [["F-AAA-01", "PASS"], ["F-AAA-02", "FAIL"], ["F-AAA-03", "NO_TEST"], ["F-AAA-04", "NO_TEST"]],
    );
    // F-AAA-02 failed in a complete milestone; F-AAA-03 is S; F-AAA-04's milestone is not complete.
    assert.deepEqual(enforcement(catalog, trace), ["F-AAA-02 (M, M1 complete): FAIL"]);
  });

  it("fails the gate when a feature of a complete milestone has no test", () => {
    const trace = buildTrace(catalog, [test("passes", ["F-AAA-02"])], [], results);
    assert.deepEqual(enforcement(catalog, trace), ["F-AAA-01 (M, M1 complete): NO_TEST"]);
  });

  it("reports IDs the docs do not define and tests that never ran", () => {
    const trace = buildTrace(catalog, [test("passes", ["F-ZZZ-99"]), test("missing", ["F-AAA-01"])], [{ file: "a.rs", ids: ["SR-999"] }], results);
    assert.deepEqual(trace.unknownIds.map((u) => u.id).sort(), ["F-ZZZ-99", "SR-999"]);
    assert.deepEqual(trace.notRun, ["tools/ci/test/x.test.ts › missing"]);
  });

  it("expects OS-gated tests only on their platform and never lets them decide a row", () => {
    const gated: TestRef = { ...test("missing", ["F-AAA-01"]), platform: "windows" };
    const elsewhere = buildTrace(catalog, [test("passes", ["F-AAA-01"]), gated], [], results, "linux");
    assert.deepEqual(elsewhere.notRun, []);
    assert.equal(elsewhere.rows.find((r) => r.id === "F-AAA-01")?.result, "PASS");
    const here = buildTrace(catalog, [test("passes", ["F-AAA-01"]), gated], [], results, "windows");
    assert.deepEqual(here.notRun, ["tools/ci/test/x.test.ts › missing"]);
    assert.equal(toCsv(here), toCsv(elsewhere), "the CSV does not depend on the platform");
  });

  it("writes CSV with the documented columns and quotes fields with commas", () => {
    const trace = buildTrace(catalog, [test("passes", ["F-AAA-01"])], [{ file: "a, b.rs", ids: ["F-AAA-01"] }], results);
    const [header, first] = toCsv(trace).split("\n");
    assert.equal(header, "requirement_id,feature_id,milestone,code_module,test_names,result");
    assert.equal(first, 'F-AAA-01,F-AAA-01,M1,"a, b.rs",tools/ci/test/x.test.ts › passes,PASS');
  });
});
