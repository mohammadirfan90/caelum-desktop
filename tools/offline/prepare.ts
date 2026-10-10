// Prepare the offline tool cache (ONLINE, on a trusted machine) — docs/OFFLINE_BUILD.md §3.1.
//   node tools/offline/prepare.ts --cache <dir>      (or set CAELUM_TOOL_CACHE)
// If Tauri's bundler tools (NSIS, WiX) are not on this machine yet, it runs one online
// `pnpm tauri build` so the Tauri CLI downloads them. Every download is checked against the digest
// its upstream publishes; the cache ends with ITEMS.json (provenance) and MANIFEST.sha256.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

import {
  type CacheItem,
  cacheDir,
  copyInto,
  downloadFile,
  downloadVerified,
  hashFile,
  writeItems,
  writeManifest,
} from "../ci/src/offline.ts";
import { REPO_ROOT } from "../ci/src/repo.ts";
import { authenticode, installRuntime, signatureProblem, WEBVIEW2 } from "../ci/src/webview2.ts";

/** Pinned versions; keep in step with rust-toolchain.toml, package.json and the installed tools. */
const RUST = "1.99.0";
const RUST_TARGET = "x86_64-pc-windows-msvc";
const NODE = "24.11.0";
const PNPM = "10.33.4";
const CARGO_TOOLS = ["cargo-nextest", "cargo-deny", "cargo-audit", "cargo-cyclonedx"] as const;

const cache = cacheDir(process.argv.slice(2));
if (cache.startsWith(REPO_ROOT)) throw new Error("the tool cache must live outside the repository");
const now = new Date().toISOString();
const items: CacheItem[] = [];

/** Runs a fixed command string (no external input), in the repository unless `cwd` is given. */
function sh(command: string, cwd: string = REPO_ROOT): string {
  console.log(`prepare: ${command}`);
  const r = spawnSync(command, {
    cwd,
    encoding: "utf8",
    shell: true,
    maxBuffer: 256 * 1024 * 1024,
    env: { ...process.env, CI: "true" },
  });
  if (r.status !== 0) {
    const out = `${r.stdout}\n${r.stderr}`.trim().split(/\r?\n/).slice(-20).join("\n");
    throw new Error(`failed (${String(r.status)}): ${command}\n${out}`);
  }
  return r.stdout;
}

async function text(url: string): Promise<string> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`download failed: ${url} (${String(response.status)})`);
  return response.text();
}

rmSync(cache, { recursive: true, force: true });
mkdirSync(cache, { recursive: true });

// 1. Rust crates: every crate in Cargo.lock, all platforms.
sh(`cargo vendor --locked --versioned-dirs ${JSON.stringify(join(cache, "vendor"))}`);
items.push({ name: "crates (cargo vendor)", version: "Cargo.lock", path: "vendor", source: "https://crates.io", fetchedAt: now, licence: "per crate; see SBOM and cargo deny", upstreamCheck: "Cargo.lock checksums (cargo vendor)" });

// 2. npm packages: the pnpm store for the whole workspace lockfile. `pnpm fetch` needs only the
// lockfile and workspace manifests; it runs in a throwaway copy so the working tree's
// node_modules are never touched.
const fetchDir = join(cache, ".pnpm-fetch");
for (const file of ["pnpm-lock.yaml", "pnpm-workspace.yaml", "package.json", "ui/package.json", "tools/ci/package.json"]) {
  copyInto(join(REPO_ROOT, file), join(fetchDir, file));
}
sh(`pnpm fetch --store-dir ${JSON.stringify(join(cache, "pnpm-store"))}`, fetchDir);
rmSync(fetchDir, { recursive: true, force: true });
// The store records the projects that used it as links; they are not needed offline and would
// dangle (and must not leave the machine).
for (const version of readdirSync(join(cache, "pnpm-store"))) {
  rmSync(join(cache, "pnpm-store", version, "projects"), { recursive: true, force: true });
}
items.push({ name: "npm packages (pnpm fetch)", version: "pnpm-lock.yaml", path: "pnpm-store", source: "https://registry.npmjs.org", fetchedAt: now, licence: "per package; see SBOM and npm licence guard", upstreamCheck: "pnpm-lock.yaml integrity hashes (pnpm)" });

// 3. Rust standalone installer, checked against the published .sha256.
const rustMsi = `rust-${RUST}-${RUST_TARGET}.msi`;
const rustUrl = `https://static.rust-lang.org/dist/${rustMsi}`;
const rustSha = (await text(`${rustUrl}.sha256`)).split(/\s+/)[0] ?? "";
await downloadVerified(rustUrl, join(cache, "rust", rustMsi), { algorithm: "sha256", digest: rustSha, encoding: "hex" });
items.push({ name: "Rust toolchain (standalone installer)", version: RUST, path: `rust/${rustMsi}`, source: rustUrl, fetchedAt: now, licence: "MIT OR Apache-2.0", upstreamCheck: `sha256 from ${rustUrl}.sha256` });

// 4. Node.js installer, checked against SHASUMS256.txt.
const nodeMsi = `node-v${NODE}-x64.msi`;
const nodeBase = `https://nodejs.org/dist/v${NODE}`;
const nodeSha = (await text(`${nodeBase}/SHASUMS256.txt`)).split(/\r?\n/).find((l) => l.endsWith(`  ${nodeMsi}`))?.split(/\s+/)[0] ?? "";
await downloadVerified(`${nodeBase}/${nodeMsi}`, join(cache, "node", nodeMsi), { algorithm: "sha256", digest: nodeSha, encoding: "hex" });
items.push({ name: "Node.js (installer)", version: NODE, path: `node/${nodeMsi}`, source: `${nodeBase}/${nodeMsi}`, fetchedAt: now, licence: "MIT", upstreamCheck: `sha256 from ${nodeBase}/SHASUMS256.txt` });

// 5. pnpm package, checked against the registry's sha512 integrity.
const pnpmMeta = JSON.parse(await text(`https://registry.npmjs.org/pnpm/${PNPM}`)) as { dist: { tarball: string; integrity: string } };
const pnpmDigest = pnpmMeta.dist.integrity.replace(/^sha512-/, "");
await downloadVerified(pnpmMeta.dist.tarball, join(cache, "pnpm", `pnpm-${PNPM}.tgz`), { algorithm: "sha512", digest: pnpmDigest, encoding: "base64" });
items.push({ name: "pnpm (npm package)", version: PNPM, path: `pnpm/pnpm-${PNPM}.tgz`, source: pnpmMeta.dist.tarball, fetchedAt: now, licence: "MIT", upstreamCheck: "sha512 integrity from registry.npmjs.org" });

// 6. Cargo tools used by the checks (installed with `cargo install --locked`).
for (const tool of CARGO_TOOLS) {
  const exe = join(homedir(), ".cargo", "bin", `${tool}.exe`);
  copyInto(exe, join(cache, "cargo-tools", `${tool}.exe`));
  const version = sh(`${tool.replace(/^cargo-/, "cargo ")} --version`).trim().split(/\r?\n/)[0] ?? "";
  items.push({ name: tool, version, path: `cargo-tools/${tool}.exe`, source: `crates.io: cargo install --locked ${tool}`, fetchedAt: now, licence: "MIT OR Apache-2.0" });
}

// 7. WebView2 Fixed Version runtime (CR-004): Microsoft publishes no digest, so the package must
// carry a valid Authenticode signature from Microsoft Corporation; its SHA-256 is recorded.
const webviewCab = join(cache, "webview2", WEBVIEW2.cab);
await downloadFile(WEBVIEW2.url, webviewCab);
const webviewSig = authenticode(webviewCab);
const webviewProblem = signatureProblem(webviewSig);
if (webviewProblem !== undefined) throw new Error(`${WEBVIEW2.cab}: ${webviewProblem}`);
items.push({
  name: "WebView2 Fixed Version runtime (x64)",
  version: WEBVIEW2.version,
  path: `webview2/${WEBVIEW2.cab}`,
  source: WEBVIEW2.url,
  fetchedAt: now,
  licence: "Microsoft WebView2 Runtime redistribution terms",
  upstreamCheck: `Authenticode ${webviewSig.status}: ${webviewSig.subject.split(",")[0] ?? ""}, issuer ${webviewSig.issuer.split(",")[0] ?? ""}; sha256 ${await hashFile(webviewCab)}`,
});

// 8. Tauri bundler tools (NSIS, WiX). The Tauri CLI downloads them on the first installer build;
// on a fresh machine run that one online build here (it needs the WebView2 runtime from step 7).
const tauriTools = join(process.env["LOCALAPPDATA"] ?? "", "tauri");
const haveTools = existsSync(join(tauriTools, "NSIS")) && existsSync(join(tauriTools, "WixTools314"));
if (!haveTools) {
  installRuntime(webviewCab, true);
  sh("pnpm tauri build");
}
copyInto(tauriTools, join(cache, "tauri-tools"));
// Tauri's own WebView2 offline-installer download (`x64/`, `x86/`, `arm64/`) is not used: CAELUM
// ships the Fixed Version runtime instead (CR-004).
for (const arch of ["x64", "x86", "arm64"]) rmSync(join(cache, "tauri-tools", arch), { recursive: true, force: true });
items.push({ name: "Tauri bundler tools (NSIS, WiX)", version: "as downloaded by @tauri-apps/cli 2.12.1", path: "tauri-tools", source: "downloaded by the Tauri CLI (GitHub releases)", fetchedAt: now, licence: "NSIS: zlib; WiX: MS-RL" });

// 9. Advisory database snapshots for air-gapped checks.
sh("cargo deny fetch");
sh("cargo audit --deny warnings");
copyInto(join(homedir(), ".cargo", "advisory-dbs"), join(cache, "advisory-db", "cargo-deny"));
copyInto(join(homedir(), ".cargo", "advisory-db"), join(cache, "advisory-db", "cargo-audit"));
const snapshotDate = now.slice(0, 10);
items.push({ name: "RustSec advisory database (snapshot)", version: snapshotDate, path: "advisory-db", source: "https://github.com/RustSec/advisory-db", fetchedAt: now, licence: "CC0-1.0" });

writeItems(cache, items);
const files = await writeManifest(cache);
console.log(`prepare: ${String(items.length)} items, ${String(files)} files, manifest written to ${join(cache, "MANIFEST.sha256")}`);
console.log(`prepare: advisory snapshot date ${snapshotDate}; ${readFileSync(join(cache, "ITEMS.json"), "utf8").length} bytes of provenance`);
