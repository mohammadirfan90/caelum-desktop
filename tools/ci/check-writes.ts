// CLI: fails on any unregistered device write or any write in caelum-vehicle (CLAUDE.md rule 5).
import { REPO_ROOT } from "./src/repo.ts";
import { checkWrites, readRegister, readSources } from "./src/writes.ts";

const files = readSources(REPO_ROOT);
const register = readRegister(REPO_ROOT);
const findings = checkWrites(files, register);
if (findings.length > 0) {
  console.error("write-exception guard: violations:");
  for (const f of findings) console.error(`  ${f.where}: ${f.message}`);
  process.exit(1);
}
console.log(
  `write-exception guard: ${String(files.length)} source files checked, ` +
    `${String(register.length)} register entries, no unregistered writes`,
);
