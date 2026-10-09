// CLI: fails if the release app's dependency graph contains test equipment (CLAUDE.md rule 3).
import { findViolations, releaseTree } from "./src/release-features.ts";

const entries = releaseTree();
const problems = findViolations(entries);
if (problems.length > 0) {
  console.error("release-feature guard: test equipment reaches the release build:");
  for (const p of problems) console.error(`  ${p}`);
  process.exit(1);
}
console.log(`release-feature guard: ${String(entries.length)} packages checked, no test equipment`);
