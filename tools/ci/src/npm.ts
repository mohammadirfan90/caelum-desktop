// npm supply-chain helpers: licence policy for the packages shipped in the UI bundle and a
// CycloneDX SBOM for them (docs/09_VV_PLAN.md §4, CR-003 §4).
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";

import { REPO_ROOT } from "./repo.ts";

/** Licences compatible with distributing CAELUM under Apache-2.0; same list as deny.toml. */
export const ALLOWED_LICENSES: ReadonlySet<string> = new Set([
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "MIT",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-3.0",
  "Zlib",
]);

/** Font licence allowed only for bundled font packages (attributed in NOTICE). */
export const FONT_LICENSE = "OFL-1.1";
const FONT_PACKAGE = /^@fontsource\//;

export interface NpmPackage {
  readonly name: string;
  readonly version: string;
  readonly license: string;
}

/** Whether an SPDX expression (simple AND / OR forms) is satisfied by the allowed set. */
export function licenseAllowed(expression: string, allowed: ReadonlySet<string>): boolean {
  const clean = expression.replace(/[()]/g, "").trim();
  if (clean.includes(" AND ")) return clean.split(" AND ").every((part) => licenseAllowed(part, allowed));
  return clean.split(" OR ").some((part) => allowed.has(part.trim()));
}

/** Returns one message per package whose licence is not allowed. */
export function checkNpmLicenses(packages: readonly NpmPackage[]): string[] {
  return packages
    .filter((p) => {
      if (FONT_PACKAGE.test(p.name) && p.license === FONT_LICENSE) return false;
      return !licenseAllowed(p.license, ALLOWED_LICENSES);
    })
    .map((p) => `${p.name}@${p.version}: licence "${p.license}" is not allowed`);
}

interface PnpmLicenseEntry {
  readonly name: string;
  readonly versions: readonly string[];
}

/** Parses `pnpm licenses list --json` output into one entry per name and version. */
export function parsePnpmLicenses(json: string): NpmPackage[] {
  const byLicense = JSON.parse(json) as Record<string, readonly PnpmLicenseEntry[]>;
  return Object.entries(byLicense)
    .flatMap(([license, entries]) =>
      entries.flatMap((e) => e.versions.map((version) => ({ name: e.name, version, license }))),
    )
    .sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`));
}

/** The production (shipped) dependencies of the UI package. */
export function shippedUiPackages(): NpmPackage[] {
  // Fixed command string (no external input): a shell is needed to run pnpm.cmd on Windows.
  const result = spawnSync("pnpm -C ui licenses list --prod --json", {
    cwd: REPO_ROOT,
    encoding: "utf8",
    shell: true,
  });
  if (result.status !== 0) throw new Error(`pnpm licenses list failed:\n${result.stderr}`);
  return parsePnpmLicenses(result.stdout);
}

export function purl(name: string, version: string): string {
  return `pkg:npm/${name.replace(/^@/, "%40")}@${version}`;
}

/** CycloneDX 1.6 JSON SBOM for the shipped UI packages. */
export function cycloneDx(app: { name: string; version: string }, packages: readonly NpmPackage[]) {
  return {
    bomFormat: "CycloneDX",
    specVersion: "1.6",
    serialNumber: `urn:uuid:${randomUUID()}`,
    version: 1,
    metadata: {
      timestamp: new Date().toISOString(),
      component: { type: "application", name: app.name, version: app.version, "bom-ref": "ui" },
    },
    components: packages.map((p) => ({
      type: "library",
      "bom-ref": purl(p.name, p.version),
      name: p.name,
      version: p.version,
      purl: purl(p.name, p.version),
      licenses: [{ expression: p.license }],
    })),
  };
}
