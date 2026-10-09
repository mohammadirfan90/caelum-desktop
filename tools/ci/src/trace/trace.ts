// trace: ignore-ids (this file documents the ID formats with examples; it cites none)
// Builds docs/trace.csv (docs/09_VV_PLAN.md §3): requirement ID → feature ID → milestone → code
// module → test names → result, and enforces it for milestones marked `complete`.
import type { Catalog, CatalogEntry, Kind } from "./catalog.ts";
import type { CaseResult } from "./junit.ts";
import { type ModuleRef, type TestRef, titlePattern } from "./scan.ts";

export type TestStatus = "PASS" | "FAIL" | "SKIPPED" | "NOT_RUN";
export type RowResult = "PASS" | "FAIL" | "NOT_RUN" | "NO_TEST";

export interface Results {
  readonly nextest: readonly CaseResult[];
  readonly vitest: readonly CaseResult[];
  /** node:test cases, classname = repository-relative test file. */
  readonly node: readonly CaseResult[];
}

function lastSegment(path: string): string {
  return path.split("::").pop() ?? path;
}

function stem(file: string): string {
  return (file.split("/").pop() ?? file).replace(/\.\w+$/, "");
}

/** The JUnit cases that belong to one scanned test. */
export function casesFor(test: TestRef, results: Results): CaseResult[] {
  switch (test.runner) {
    case "nextest": {
      const integration = test.file.includes("/tests/");
      return results.nextest.filter((c) => {
        const [crate, binary] = c.classname.split("::");
        if (crate !== test.unit || lastSegment(c.name) !== test.name) return false;
        return integration ? binary === stem(test.file) : binary === undefined || binary.startsWith("bin/");
      });
    }
    case "vitest": {
      const file = test.file.replace(/^ui\//, "");
      const pattern = titlePattern(test.name);
      return results.vitest.filter((c) => c.classname === file && pattern.test(c.name));
    }
    case "node": {
      const pattern = titlePattern(test.name);
      return results.node.filter((c) => c.classname === test.file && pattern.test(c.name));
    }
  }
}

export function testStatus(cases: readonly CaseResult[]): TestStatus {
  if (cases.length === 0) return "NOT_RUN";
  if (cases.some((c) => c.outcome === "failed")) return "FAIL";
  if (cases.every((c) => c.outcome === "skipped")) return "SKIPPED";
  return "PASS";
}

export function testLabel(test: TestRef): string {
  return test.runner === "nextest" ? `${test.unit}::${test.name}` : `${test.file} › ${test.name}`;
}

export interface TraceRow {
  readonly id: string;
  readonly featureIds: readonly string[];
  readonly milestones: readonly string[];
  readonly modules: readonly string[];
  readonly tests: readonly string[];
  readonly result: RowResult;
}

export interface Trace {
  readonly rows: readonly TraceRow[];
  /** IDs cited in code or tests that the docs do not define (typos or removed IDs). */
  readonly unknownIds: readonly { id: string; where: string }[];
  /** Scanned tests with no reported result (scanner and runner disagree). */
  readonly notRun: readonly string[];
}

const KIND_ORDER: Record<Kind, number> = { feature: 0, requirement: 1, safety: 2, need: 3 };

function idOrder(a: CatalogEntry, b: CatalogEntry): number {
  return KIND_ORDER[a.kind] - KIND_ORDER[b.kind] || a.id.localeCompare(b.id);
}

export function buildTrace(catalog: Catalog, tests: readonly TestRef[], modules: readonly ModuleRef[], results: Results): Trace {
  const status = new Map(tests.map((t) => [t, testStatus(casesFor(t, results))]));
  const unknownIds: { id: string; where: string }[] = [];
  for (const t of tests) for (const id of t.ids) if (!catalog.entries.has(id)) unknownIds.push({ id, where: testLabel(t) });
  for (const m of modules) for (const id of m.ids) if (!catalog.entries.has(id)) unknownIds.push({ id, where: m.file });

  const rows = [...catalog.entries.values()].sort(idOrder).map((entry): TraceRow => {
    const cited = tests.filter((t) => t.ids.includes(entry.id));
    const statuses = cited.map((t) => status.get(t) ?? "NOT_RUN");
    const result: RowResult =
      cited.length === 0
        ? "NO_TEST"
        : statuses.includes("FAIL")
          ? "FAIL"
          : statuses.includes("NOT_RUN")
            ? "NOT_RUN"
            : statuses.includes("PASS")
              ? "PASS"
              : "NO_TEST";
    return {
      id: entry.id,
      featureIds: entry.kind === "feature" ? [entry.id] : entry.features,
      milestones: entry.milestones,
      modules: [...new Set(modules.filter((m) => m.ids.includes(entry.id)).map((m) => m.file))].sort(),
      tests: [...new Set(cited.map(testLabel))].sort(),
      result,
    };
  });

  const notRun = tests.filter((t) => status.get(t) === "NOT_RUN").map(testLabel).sort();
  return { rows, unknownIds, notRun };
}

/** 09 §3: an M-priority feature of a milestone marked `complete` must have a passing test. */
export function enforcement(catalog: Catalog, trace: Trace): string[] {
  const complete = new Set([...catalog.milestoneStatus].filter(([, s]) => s === "complete").map(([m]) => m));
  const problems: string[] = [];
  for (const row of trace.rows) {
    const entry = catalog.entries.get(row.id);
    if (entry?.kind !== "feature" || entry.priority !== "M") continue;
    const done = row.milestones.filter((m) => complete.has(m));
    if (done.length > 0 && row.result !== "PASS") {
      problems.push(`${row.id} (M, ${done.join(", ")} complete): ${row.result}`);
    }
  }
  return problems;
}

function csvField(value: string): string {
  return /[",\n]/.test(value) ? `"${value.replaceAll('"', '""')}"` : value;
}

export const CSV_HEADER = ["requirement_id", "feature_id", "milestone", "code_module", "test_names", "result"];

export function toCsv(trace: Trace): string {
  const lines = [CSV_HEADER.join(",")];
  for (const row of trace.rows) {
    lines.push(
      [row.id, row.featureIds.join("; "), row.milestones.join("; "), row.modules.join("; "), row.tests.join("; "), row.result]
        .map(csvField)
        .join(","),
    );
  }
  return `${lines.join("\n")}\n`;
}
