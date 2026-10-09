// Offline tool cache (R-OPS-03, F-SYS-06, docs/OFFLINE_BUILD.md): manifest of SHA-256 checksums,
// provenance records, verified downloads and copies.
import { createHash } from "node:crypto";
import { cpSync, createReadStream, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";

export const MANIFEST = "MANIFEST.sha256";
export const ITEMS = "ITEMS.json";

/** Provenance of one cache item (docs/OFFLINE_BUILD.md §2). */
export interface CacheItem {
  readonly name: string;
  readonly version: string;
  /** Repository-independent path inside the cache. */
  readonly path: string;
  readonly source: string;
  readonly fetchedAt: string;
  readonly licence: string;
  /** How the download was checked against its upstream, if it was a download. */
  readonly upstreamCheck?: string;
}

export async function hashFile(path: string, algorithm: "sha256" | "sha512" = "sha256"): Promise<string> {
  const hash = createHash(algorithm);
  for await (const chunk of createReadStream(path)) hash.update(chunk as Buffer);
  return hash.digest("hex");
}

/**
 * Every regular file below `root`, as sorted forward-slash paths relative to it, plus every
 * symbolic link or junction found. A cache that moves between machines must contain no links:
 * they can dangle or point outside the cache.
 */
export function scanTree(root: string): { files: string[]; links: string[] } {
  const files: string[] = [];
  const links: string[] = [];
  const walk = (dir: string): void => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      const rel = relative(root, path).replaceAll("\\", "/");
      const info = lstatSync(path);
      if (info.isSymbolicLink()) links.push(rel);
      else if (info.isDirectory()) walk(path);
      else files.push(rel);
    }
  };
  walk(root);
  return { files: files.sort(), links: links.sort() };
}

export function listFiles(root: string): string[] {
  return scanTree(root).files;
}

/** `sha256sum`-compatible manifest of every file in the cache except the manifest itself. */
export async function writeManifest(cache: string): Promise<number> {
  const { links } = scanTree(cache);
  if (links.length > 0) throw new Error(`links are not allowed in the tool cache: ${links.join(", ")}`);
  const files = listFiles(cache).filter((f) => f !== MANIFEST);
  const lines: string[] = [];
  for (const file of files) lines.push(`${await hashFile(join(cache, file))}  ${file}`);
  writeFileSync(join(cache, MANIFEST), `${lines.join("\n")}\n`);
  return files.length;
}

export function parseManifest(text: string): Map<string, string> {
  const entries = new Map<string, string>();
  for (const line of text.split(/\r?\n/).filter((l) => l.trim().length > 0)) {
    const m = /^([0-9a-f]{64}) [ *](.+)$/.exec(line);
    if (m?.[1] === undefined || m[2] === undefined) throw new Error(`malformed manifest line: ${line}`);
    entries.set(m[2], m[1]);
  }
  return entries;
}

/** Problems found when checking the cache against its manifest; empty when the cache is intact. */
export async function verifyCache(cache: string): Promise<string[]> {
  const manifestPath = join(cache, MANIFEST);
  if (!existsSync(manifestPath)) return [`${MANIFEST} not found in ${cache}`];
  const expected = parseManifest(readFileSync(manifestPath, "utf8"));
  const tree = scanTree(cache);
  const actual = new Set(tree.files.filter((f) => f !== MANIFEST));
  const problems: string[] = tree.links.map((l) => `link not allowed: ${l}`);
  for (const [file, sha] of expected) {
    if (!actual.has(file)) problems.push(`missing: ${file}`);
    else if ((await hashFile(join(cache, file))) !== sha) problems.push(`checksum mismatch: ${file}`);
  }
  for (const file of actual) if (!expected.has(file)) problems.push(`not in manifest: ${file}`);
  return problems;
}

/** Downloads `url` to `dest` and checks it against an upstream digest (hex or base64). */
export async function downloadVerified(
  url: string,
  dest: string,
  expected: { algorithm: "sha256" | "sha512"; digest: string; encoding: "hex" | "base64" },
): Promise<void> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`download failed: ${url} (${String(response.status)})`);
  mkdirSync(dirname(dest), { recursive: true });
  writeFileSync(dest, Buffer.from(await response.arrayBuffer()));
  const hex = await hashFile(dest, expected.algorithm);
  const actual = expected.encoding === "hex" ? hex : Buffer.from(hex, "hex").toString("base64");
  if (actual !== expected.digest) {
    throw new Error(`${url}: ${expected.algorithm} mismatch (expected ${expected.digest}, got ${actual})`);
  }
}

/** Copies a file or directory into the cache. */
export function copyInto(source: string, dest: string): void {
  if (!existsSync(source)) throw new Error(`not found: ${source}`);
  mkdirSync(dirname(dest), { recursive: true });
  cpSync(source, dest, { recursive: true });
}

export function writeItems(cache: string, items: readonly CacheItem[]): void {
  writeFileSync(join(cache, ITEMS), `${JSON.stringify(items, null, 2)}\n`);
}

export function readItems(cache: string): CacheItem[] {
  return JSON.parse(readFileSync(join(cache, ITEMS), "utf8")) as CacheItem[];
}

/** The cache directory from `--cache <dir>` or CAELUM_TOOL_CACHE. */
export function cacheDir(argv: readonly string[]): string {
  const at = argv.indexOf("--cache");
  const dir = at >= 0 ? argv[at + 1] : process.env["CAELUM_TOOL_CACHE"];
  if (dir === undefined || dir.length === 0) {
    throw new Error("set CAELUM_TOOL_CACHE or pass --cache <dir> (outside the repository)");
  }
  return dir;
}

/** Cargo config that builds from the vendored crates only, with no network. */
export function offlineCargoConfig(vendorDir: string): string {
  const dir = vendorDir.replaceAll("\\", "/");
  return [
    "# Generated by tools/offline/build-offline.ts: crates come only from the tool cache.",
    "[source.crates-io]",
    'replace-with = "vendored-sources"',
    "",
    "[source.vendored-sources]",
    `directory = "${dir}"`,
    "",
    "[net]",
    "offline = true",
    "",
  ].join("\n");
}
