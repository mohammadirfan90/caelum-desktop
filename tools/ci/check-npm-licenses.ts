// CLI: fails if a package shipped in the UI bundle has a licence outside the CR-003 policy.
import { checkNpmLicenses, shippedUiPackages } from "./src/npm.ts";

const packages = shippedUiPackages();
const problems = checkNpmLicenses(packages);
if (problems.length > 0) {
  console.error("npm licence guard: licences not allowed in the shipped UI:");
  for (const p of problems) console.error(`  ${p}`);
  process.exit(1);
}
console.log(`npm licence guard: ${String(packages.length)} shipped UI packages, all licences allowed`);
