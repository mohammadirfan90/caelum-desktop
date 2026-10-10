// Runs every check of docs/09_VV_PLAN.md §4 that can run on a developer machine, in CI order,
// and prints a summary. Same commands as .github/workflows/ci.yml.
//   node tools/ci/check-all.ts             online (advisory databases and pnpm audit fetched)
//   node tools/ci/check-all.ts --offline   air-gapped: advisory snapshot, no pnpm audit; the
//                                          browser-mode UI tests use Chromium from the tool cache
//                                          ($CAELUM_TOOL_CACHE/playwright, docs/OFFLINE_BUILD.md)
// Exit code 1 if any check fails.
import { spawnSync } from "node:child_process";
import { join } from "node:path";

import { REPO_ROOT } from "./src/repo.ts";

const offline = process.argv.includes("--offline");
const toolCache = process.env.CAELUM_TOOL_CACHE;
const offlineEnv: NodeJS.ProcessEnv = {
  ...process.env,
  CARGO_NET_OFFLINE: "true",
  ...(toolCache === undefined || process.env.PLAYWRIGHT_BROWSERS_PATH !== undefined ? {} : { PLAYWRIGHT_BROWSERS_PATH: join(toolCache, "playwright") }),
};

interface Check {
  readonly name: string;
  readonly command: string;
}

const CHECKS: readonly Check[] = [
  { name: "rustfmt", command: "cargo fmt --all --check" },
  { name: "clippy (-D warnings)", command: "cargo clippy --workspace --all-targets -- -D warnings" },
  { name: "UI typecheck", command: "pnpm -C ui typecheck" },
  { name: "UI lint", command: "pnpm -C ui lint" },
  { name: "UI build + offline bundle check", command: "pnpm -C ui build" },
  { name: "tools typecheck", command: "pnpm -C tools/ci typecheck" },
  { name: "guards (release features, writes, npm licences)", command: "pnpm -C tools/ci check" },
  { name: "cargo deny", command: offline ? "cargo deny --offline check" : "cargo deny check" },
  { name: "cargo audit", command: offline ? "cargo audit --deny warnings --no-fetch" : "cargo audit --deny warnings" },
  ...(offline ? [] : [{ name: "pnpm audit", command: "pnpm audit" }]),
  { name: "all test suites + trace (trace:check)", command: "pnpm -C tools/ci trace:check" },
  { name: "SBOMs", command: "pnpm -C tools/ci sbom" },
];

const results: { name: string; ok: boolean; seconds: number }[] = [];
for (const check of CHECKS) {
  console.log(`\n=== ${check.name}: ${check.command}`);
  const start = Date.now();
  // Fixed command strings (no external input); a shell is needed for pnpm.cmd on Windows.
  const r = spawnSync(check.command, {
    cwd: REPO_ROOT,
    stdio: "inherit",
    shell: true,
    env: offline ? offlineEnv : process.env,
  });
  results.push({ name: check.name, ok: r.status === 0, seconds: Math.round((Date.now() - start) / 1000) });
}

console.log(`\ncheck-all (${offline ? "offline" : "online"}):`);
for (const r of results) console.log(`  ${r.ok ? "PASS" : "FAIL"}  ${r.name} (${String(r.seconds)} s)`);
const failed = results.filter((r) => !r.ok).length;
console.log(failed === 0 ? "check-all: all checks passed" : `check-all: ${String(failed)} check(s) failed`);
process.exit(failed === 0 ? 0 : 1);
