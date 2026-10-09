// CycloneDX SBOMs for a release (docs/09_VV_PLAN.md §4): the app's Rust dependencies for one
// target (via cargo-cyclonedx) and the npm packages shipped in the UI bundle.
import { existsSync, mkdirSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { cycloneDx, shippedUiPackages } from "./npm.ts";
import { cargo, REPO_ROOT } from "./repo.ts";

export const SBOM_DIR = join(REPO_ROOT, "target", "sbom");
const GENERATED = "caelum-desktop.cdx.json";

/** The host target triple, e.g. x86_64-pc-windows-msvc. */
export function hostTarget(): string {
  const host = /^host: (.+)$/m.exec(cargo(["--version", "--verbose"]))?.[1];
  if (host === undefined) throw new Error("cannot determine the host target from cargo --version --verbose");
  return host.trim();
}

/** Writes both SBOMs into target/sbom/ and returns their paths. */
export function writeSboms(target: string = hostTarget()): { rust: string; ui: string } {
  mkdirSync(SBOM_DIR, { recursive: true });

  // Shipped (normal) dependencies only, for one target: build tools are not in the installer and
  // a Windows SBOM must not list Linux-only crates.
  cargo([
    "cyclonedx",
    "--manifest-path", join(REPO_ROOT, "src-tauri", "Cargo.toml"),
    "--format", "json",
    "--spec-version", "1.5",
    "--no-build-deps",
    "--target", target,
    "--override-filename", GENERATED.replace(/\.json$/, ""),
  ]);
  // cargo-cyclonedx writes one file next to every workspace manifest; keep only the app's,
  // which covers its whole dependency tree, and remove the rest.
  const rust = join(SBOM_DIR, `caelum-desktop-${target}.cdx.json`);
  renameSync(join(REPO_ROOT, "src-tauri", GENERATED), rust);
  for (const crate of readdirSync(join(REPO_ROOT, "crates"))) {
    const stray = join(REPO_ROOT, "crates", crate, GENERATED);
    if (existsSync(stray)) rmSync(stray);
  }

  const ui = join(SBOM_DIR, "caelum-ui.cdx.json");
  const bom = cycloneDx({ name: "@caelum/ui", version: "0.1.0" }, shippedUiPackages());
  writeFileSync(ui, `${JSON.stringify(bom, null, 2)}\n`);
  return { rust, ui };
}
