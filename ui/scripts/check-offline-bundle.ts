// Fails the UI build if the bundle would load anything from the network (CLAUDE.md rule 8,
// R-OPS-03, docs/03_ARCHITECTURE.md §5: no remote content). Runs after `vite build`.
//
// HTML and CSS may not reference any http(s) URL. JavaScript may contain only the inert URL
// strings listed below (namespaces and documentation links that are never fetched); anything
// else fails the build so it gets reviewed.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const DIST = fileURLToPath(new URL("../dist", import.meta.url));

/** URL prefixes that appear as plain strings in bundled libraries and are never requested. */
const INERT_JS_URL_PREFIXES: readonly string[] = [
  "http://www.w3.org/", // XML/SVG/MathML namespace identifiers (React DOM)
  "https://react.dev/errors/", // React production error decoder link, shown in error text only
  "http://ipc.localhost", // Tauri IPC endpoint on Windows (local, allowed by the CSP)
];

const URL_PATTERN = /https?:\/\/[^\s"'`)<>\\]+/g;

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

const problems: string[] = [];
let checked = 0;
for (const path of files(DIST)) {
  const ext = extname(path);
  if (![".html", ".css", ".js"].includes(ext)) continue;
  checked += 1;
  const raw = readFileSync(path, "utf8");
  // CSS comments (e.g. the Tailwind MIT licence banner, which must be kept) can never load
  // anything; every url() and @import outside them is still checked.
  const text = ext === ".css" ? raw.replace(/\/\*[\s\S]*?\*\//g, "") : raw;
  for (const match of text.matchAll(URL_PATTERN)) {
    const url = match[0];
    const inert = ext === ".js" && INERT_JS_URL_PREFIXES.some((prefix) => url.startsWith(prefix));
    if (!inert) problems.push(`${relative(DIST, path)}: ${url}`);
  }
}

if (checked === 0) {
  console.error(`check-offline-bundle: no HTML, CSS or JS found in ${DIST}; run vite build first`);
  process.exit(1);
}
if (problems.length > 0) {
  console.error("check-offline-bundle: remote URLs in the UI bundle:");
  for (const p of problems) console.error(`  ${p}`);
  process.exit(1);
}
console.log(`check-offline-bundle: ${String(checked)} files checked, no remote content`);
