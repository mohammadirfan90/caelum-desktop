// Coverage gates (docs/09 §4). Usage: node check-coverage.ts <llvm-cov summary JSON>
// Produce the input with: cargo llvm-cov nextest --workspace --json --summary-only --output-path <file>
import { readFileSync } from "node:fs";

import { evaluate, EXCLUDED, parseSummary } from "./src/coverage.ts";

const path = process.argv[2];
if (path === undefined) {
  console.error("usage: node check-coverage.ts <llvm-cov summary JSON>");
  process.exit(2);
}
const results = evaluate(parseSummary(readFileSync(path, "utf8")));
for (const r of results) {
  const value = r.percent === undefined ? "no code yet" : `${r.percent.toFixed(2)} %`;
  console.log(`coverage: ${r.gate.crate} ${r.gate.metric} ${value} (gate ${String(r.gate.min)} %) ${r.ok ? "ok" : "FAIL"}`);
}
for (const e of EXCLUDED) console.log(`coverage: excluded ${e.file}: ${e.reason}`);
const failed = results.filter((r) => !r.ok);
if (failed.length > 0) {
  console.error(`coverage gate failed for ${failed.map((r) => r.gate.crate).join(", ")}`);
  process.exit(1);
}
