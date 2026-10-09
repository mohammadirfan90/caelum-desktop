// Release-feature guard (CLAUDE.md rule 3, R-CLN-02): the dependency graph of the release app
// must not contain test equipment, neither the crate nor any `test-equipment` feature.
// Second line of defence after the compile_error! in src-tauri/src/lib.rs.
import { cargo } from "./repo.ts";

export const APP_PACKAGE = "caelum-desktop";
export const TEST_EQUIPMENT_CRATE = "caelum-test-equipment";
export const TEST_EQUIPMENT_FEATURE = "test-equipment";

export interface TreeEntry {
  readonly name: string;
  readonly features: readonly string[];
}

/** Parses `cargo tree --prefix none -f "{p}|{f}"` output. */
export function parseTree(output: string): TreeEntry[] {
  return output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => {
      const [pkg = "", features = ""] = line.split("|");
      const name = pkg.split(" ")[0] ?? "";
      return {
        name,
        features: features.split(",").map((f) => f.trim()).filter((f) => f.length > 0),
      };
    });
}

/** Returns one message per violation; empty when the graph is clean. */
export function findViolations(entries: readonly TreeEntry[]): string[] {
  const problems = new Set<string>();
  for (const entry of entries) {
    if (entry.name === TEST_EQUIPMENT_CRATE) {
      problems.add(`${TEST_EQUIPMENT_CRATE} is in the release dependency graph`);
    }
    if (entry.features.includes(TEST_EQUIPMENT_FEATURE)) {
      problems.add(`${entry.name} has the \`${TEST_EQUIPMENT_FEATURE}\` feature enabled`);
    }
  }
  return [...problems].sort();
}

/**
 * The app's normal and build dependency graph for every target, with the features a release
 * build uses (default features unless `extraFeatures` is given, which the self-tests use).
 */
export function releaseTree(extraFeatures: readonly string[] = []): TreeEntry[] {
  const args = ["tree", "-p", APP_PACKAGE, "-e", "normal,build", "--target", "all", "--prefix", "none", "-f", "{p}|{f}"];
  if (extraFeatures.length > 0) args.push("--features", extraFeatures.join(","));
  return parseTree(cargo(args));
}
