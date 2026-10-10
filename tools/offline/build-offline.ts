// Release build from the tool cache only (R-OPS-03) — docs/OFFLINE_BUILD.md §3.3.
//   node tools/offline/build-offline.ts --cache <dir>      (or set CAELUM_TOOL_CACHE)
// Run with the network disabled. The cache is verified first; crates come from the vendored copy
// (cargo offline), npm packages from the cached store (pnpm offline), and Tauri's bundler tools
// from the cache. Writes target/offline/build-report.json with installer checksums.
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { cacheDir, hashFile, offlineCargoConfig, readItems, verifyCache } from "../ci/src/offline.ts";
import { REPO_ROOT } from "../ci/src/repo.ts";
import { installRuntime, RELEASE_CONFIG, WEBVIEW2 } from "../ci/src/webview2.ts";

const cache = cacheDir(process.argv.slice(2));
const outDir = join(REPO_ROOT, "target", "offline");
mkdirSync(outDir, { recursive: true });

// 1. The cache must be intact before anything is used from it.
const problems = await verifyCache(cache);
if (problems.length > 0) {
  console.error("build-offline: tool cache is not intact; refusing to build:");
  for (const p of problems) console.error(`  ${p}`);
  process.exit(1);
}

// 2. Tauri bundler tools where the Tauri CLI looks for them (copy only what is missing).
const tauriTools = join(process.env["LOCALAPPDATA"] ?? "", "tauri");
cpSync(join(cache, "tauri-tools"), tauriTools, { recursive: true, force: false, errorOnExist: false });

// 2b. The pinned WebView2 Fixed Version runtime into src-tauri/webview2-fixed-runtime (CR-004).
const webview = installRuntime(join(cache, "webview2", WEBVIEW2.cab), false);
console.log(`build-offline: WebView2 ${WEBVIEW2.version} runtime installed (Authenticode ${webview.status})`);

// 3. Cargo: vendored crates only, no network.
const cargoConfig = join(outDir, "cargo-offline.toml");
writeFileSync(cargoConfig, offlineCargoConfig(join(cache, "vendor")));

/**
 * Network trap: every proxy-respecting client (cargo, pnpm, the Tauri bundler) is sent to a closed
 * local port, so any network access the build still attempts fails loudly instead of succeeding
 * quietly. It complements, not replaces, building with the network disabled.
 */
const DEAD_PROXY = "http://127.0.0.1:9";
const OFFLINE_ENV = {
  ...process.env,
  CARGO_NET_OFFLINE: "true",
  npm_config_offline: "true",
  // Non-interactive: lets pnpm replace node_modules installed from another store.
  CI: "true",
  HTTP_PROXY: DEAD_PROXY,
  HTTPS_PROXY: DEAD_PROXY,
  http_proxy: DEAD_PROXY,
  https_proxy: DEAD_PROXY,
  ALL_PROXY: DEAD_PROXY,
  NO_PROXY: "",
};

/** Runs a fixed command string (no external input) with offline settings and the network trap. */
function sh(command: string): void {
  console.log(`build-offline: ${command}`);
  const r = spawnSync(command, { cwd: REPO_ROOT, stdio: "inherit", shell: true, env: OFFLINE_ENV });
  if (r.status !== 0) {
    console.error(`build-offline: failed (${String(r.status)}): ${command}`);
    process.exit(1);
  }
}

// 4. npm packages from the cached store, then the release build.
const store = JSON.stringify(join(cache, "pnpm-store"));
sh(`pnpm install --offline --frozen-lockfile --store-dir ${store}`);
sh(`pnpm tauri build --config ${RELEASE_CONFIG} -- --config ${JSON.stringify(cargoConfig)}`);

// 5. Report: installers with checksums, and the cache they came from.
const bundle = join(REPO_ROOT, "target", "release", "bundle");
const installers: { file: string; sha256: string }[] = [];
for (const kind of ["msi", "nsis"]) {
  const dir = join(bundle, kind);
  if (!existsSync(dir)) continue;
  for (const name of readdirSync(dir).filter((n) => /\.(msi|exe)$/.test(n))) {
    installers.push({ file: `target/release/bundle/${kind}/${name}`, sha256: await hashFile(join(dir, name)) });
  }
}
const report = {
  builtAt: new Date().toISOString(),
  webview2: { version: WEBVIEW2.version, mode: "fixedRuntime", authenticode: webview.status },
  cache: readItems(cache).map((i) => ({ name: i.name, version: i.version, fetchedAt: i.fetchedAt })),
  installers,
};
writeFileSync(join(outDir, "build-report.json"), `${JSON.stringify(report, null, 2)}\n`);
for (const i of installers) console.log(`build-offline: ${i.sha256}  ${i.file}`);
if (installers.length === 0) {
  console.error("build-offline: no installer produced");
  process.exit(1);
}
