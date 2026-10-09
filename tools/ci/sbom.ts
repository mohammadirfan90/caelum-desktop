// CLI: writes the release CycloneDX SBOMs into target/sbom/ (docs/09_VV_PLAN.md §4).
// Optional argument: target triple (default: host).
import { writeSboms } from "./src/sbom.ts";

const { rust, ui } = writeSboms(process.argv[2]);
console.log(`sbom: wrote\n  ${rust}\n  ${ui}`);
