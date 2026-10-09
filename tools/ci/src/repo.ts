// Shared helpers for the CI guards: repository paths and running cargo.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

/** Repository root (tools/ci/src → ../../..). */
export const REPO_ROOT = fileURLToPath(new URL("../../../", import.meta.url));

/** Runs cargo in the repository root and returns stdout; throws with stderr on failure. */
export function cargo(args: readonly string[]): string {
  const result = spawnSync("cargo", [...args], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error !== undefined) throw result.error;
  if (result.status !== 0) {
    throw new Error(`cargo ${args.join(" ")} failed (${String(result.status)}):\n${result.stderr}`);
  }
  return result.stdout;
}
