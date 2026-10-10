// CLI: traceability (docs/09_VV_PLAN.md §3, CLAUDE.md rule 9).
//   node trace.ts            run all test suites with JUnit output, write docs/trace.csv
//   node trace.ts --no-run   reuse the last JUnit reports
//   node trace.ts --check    like the default, but fail if docs/trace.csv is out of date
// Exit code 1 on: failing or unmatched tests, IDs cited in code that the docs do not define, or an
// M-priority feature of a `complete` milestone without a passing test.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT } from "./src/repo.ts";
import { readCatalog } from "./src/trace/catalog.ts";
import { type CaseResult, parseJunit } from "./src/trace/junit.ts";
import { scanRepository } from "./src/trace/scan.ts";
import { buildTrace, enforcement, type Results, toCsv } from "./src/trace/trace.ts";

const args = new Set(process.argv.slice(2));
const JUNIT_DIR = join(REPO_ROOT, "target", "junit");
const NEXTEST_JUNIT = join(REPO_ROOT, "target", "nextest", "ci", "junit.xml");
const UI_JUNIT = join(JUNIT_DIR, "ui.xml");
/** UI tests in a real browser (Vitest browser mode, Chromium via Playwright; M5 gate). */
const UI_BROWSER_JUNIT = join(JUNIT_DIR, "ui-browser.xml");
const NODE_JUNIT_DIR = join(JUNIT_DIR, "tools-ci");
const NODE_TEST_DIR = join(REPO_ROOT, "tools", "ci", "test");
const CSV_PATH = join(REPO_ROOT, "docs", "trace.csv");

/** Runs a fixed command string (no external input; a shell is needed for pnpm.cmd on Windows). */
function run(command: string, cwd: string): void {
  console.log(`trace: ${command}`);
  spawnSync(command, { cwd, stdio: "ignore", shell: true });
}

function runSuites(): void {
  mkdirSync(NODE_JUNIT_DIR, { recursive: true });
  rmSync(NEXTEST_JUNIT, { force: true });
  rmSync(UI_JUNIT, { force: true });
  rmSync(UI_BROWSER_JUNIT, { force: true });
  run("cargo nextest run --workspace --profile ci", REPO_ROOT);
  run(`pnpm -C ui exec vitest run --reporter=junit --outputFile=${JSON.stringify(UI_JUNIT)}`, REPO_ROOT);
  run(`pnpm -C ui exec vitest run --config vitest.browser.config.ts --reporter=junit --outputFile=${JSON.stringify(UI_BROWSER_JUNIT)}`, REPO_ROOT);
  for (const file of readdirSync(NODE_TEST_DIR).filter((f) => f.endsWith(".test.ts"))) {
    const out = join(NODE_JUNIT_DIR, file.replace(/\.ts$/, ".xml"));
    rmSync(out, { force: true });
    run(`node --test --test-reporter=junit --test-reporter-destination=${JSON.stringify(out)} test/${file}`, join(REPO_ROOT, "tools", "ci"));
  }
}

function readJunit(path: string): CaseResult[] {
  if (!existsSync(path)) throw new Error(`missing test report ${path}; run without --no-run`);
  return parseJunit(readFileSync(path, "utf8"));
}

function readResults(): Results {
  const node = readdirSync(NODE_TEST_DIR)
    .filter((f) => f.endsWith(".test.ts"))
    .flatMap((file) =>
      readJunit(join(NODE_JUNIT_DIR, file.replace(/\.ts$/, ".xml"))).map((c) => ({ ...c, classname: `tools/ci/test/${file}` })),
    );
  return { nextest: readJunit(NEXTEST_JUNIT), vitest: [...readJunit(UI_JUNIT), ...readJunit(UI_BROWSER_JUNIT)], node };
}

if (!args.has("--no-run")) runSuites();

const catalog = readCatalog(REPO_ROOT);
const { tests, modules } = scanRepository(REPO_ROOT);
const results = readResults();
const trace = buildTrace(catalog, tests, modules, results);
const csv = toCsv(trace);

const errors: string[] = [];
const failedCases = [...results.nextest, ...results.vitest, ...results.node].filter((c) => c.outcome === "failed");
for (const c of failedCases) errors.push(`failing test: ${c.classname} ${c.name}`);
for (const u of trace.unknownIds) errors.push(`${u.id} is cited in ${u.where} but not defined in docs/`);
for (const t of trace.notRun) errors.push(`test found in source but not in any report: ${t}`);
const violations = enforcement(catalog, trace);
for (const v of violations) errors.push(`traceability gate: ${v}`);

if (args.has("--check")) {
  const committed = existsSync(CSV_PATH) ? readFileSync(CSV_PATH, "utf8").replaceAll("\r\n", "\n") : "";
  if (committed !== csv) errors.push("docs/trace.csv is out of date; run `pnpm -C tools/ci trace`");
} else {
  writeFileSync(CSV_PATH, csv);
}

const count = (r: string) => trace.rows.filter((row) => row.result === r).length;
const complete = [...catalog.milestoneStatus].filter(([, s]) => s === "complete").map(([m]) => m);
console.log(
  `trace: ${String(trace.rows.length)} IDs, ${String(tests.length)} tests | ` +
    `PASS ${String(count("PASS"))}, NO_TEST ${String(count("NO_TEST"))}, FAIL ${String(count("FAIL"))}, NOT_RUN ${String(count("NOT_RUN"))} | ` +
    `enforced milestones: ${complete.length > 0 ? complete.join(", ") : "none complete yet"}`,
);
if (errors.length > 0) {
  console.error("trace: errors:");
  for (const e of errors) console.error(`  ${e}`);
  process.exit(1);
}
