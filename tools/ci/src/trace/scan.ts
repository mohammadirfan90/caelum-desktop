// trace: ignore-ids (this file documents the ID formats with examples; it cites none)
// Finds tests and the requirement IDs they cite (CLAUDE.md rule 9: "cited in a test name or doc
// comment"). A test cites the IDs in its own name, in the comment block directly above it, and in
// its file's header comment. Non-test files that mention an ID are listed as code modules.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

import { extractIds, idsFromName } from "./ids.ts";

export type Runner = "nextest" | "vitest" | "node";

export interface TestRef {
  readonly runner: Runner;
  /** Repository-relative source file. */
  readonly file: string;
  /** Rust: crate name. TS: package directory (ui, tools/ci). */
  readonly unit: string;
  /** Rust: function name. TS: title as written (may contain %s or ${...}). */
  readonly name: string;
  readonly ids: readonly string[];
}

export interface ModuleRef {
  readonly file: string;
  readonly ids: readonly string[];
}

/** Leading comment block of a file (Rust `//!` or `//`, TS `//`). */
function headerComment(lines: readonly string[]): string {
  const out: string[] = [];
  for (const line of lines) {
    const t = line.trim();
    if (t.startsWith("//")) out.push(t);
    else if (t.length === 0 && out.length === 0) continue;
    else break;
  }
  return out.join("\n");
}

/** Comment lines directly above `index`, skipping attributes and blank-free decorators. */
function commentAbove(lines: readonly string[], index: number): string {
  const out: string[] = [];
  for (let i = index - 1; i >= 0; i -= 1) {
    const t = (lines[i] ?? "").trim();
    if (t.startsWith("#[")) continue;
    if (t.startsWith("//")) out.unshift(t);
    else break;
  }
  return out.join("\n");
}

/** Rust tests: `#[test]` (optionally followed by more attributes) then `fn name(`. */
export function scanRust(file: string, crate: string, text: string): TestRef[] {
  const lines = text.split(/\r?\n/);
  const header = headerComment(lines);
  const tests: TestRef[] = [];
  lines.forEach((line, index) => {
    if (line.trim() !== "#[test]") return;
    for (let j = index + 1; j < Math.min(lines.length, index + 6); j += 1) {
      const fn = /^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)\s*\(/.exec(lines[j] ?? "");
      if (fn?.[1] === undefined) continue;
      const name = fn[1];
      const ids = new Set([...idsFromName(name), ...extractIds(commentAbove(lines, index)), ...extractIds(header)]);
      tests.push({ runner: "nextest", file, unit: crate, name, ids: [...ids].sort() });
      return;
    }
  });
  return tests;
}

const TS_TEST =
  /\b(?:it|test)(?:\.each\([\s\S]*?\)\(\s*|\(\s*)(["'`])((?:\\.|(?!\1)[^\\])*?)\1/g;

/** TS tests: `it("title"`, `test("title"`, `it.each(...)("title"`; titles may use templates. */
export function scanTs(file: string, unit: string, runner: Runner, text: string): TestRef[] {
  const lines = text.split(/\r?\n/);
  const header = headerComment(lines);
  const tests: TestRef[] = [];
  for (const m of text.matchAll(TS_TEST)) {
    const title = m[2] ?? "";
    const line = text.slice(0, m.index).split(/\r?\n/).length - 1;
    const ids = new Set([...extractIds(title), ...extractIds(commentAbove(lines, line)), ...extractIds(header)]);
    tests.push({ runner, file, unit, name: title, ids: [...ids].sort() });
  }
  return tests;
}

/** Turns a source title into a pattern matching the reported test name. */
export function titlePattern(title: string): RegExp {
  const placeholder = "\u0000";
  const marked = title.replace(/\$\{[^}]*\}|%[sdifjo]/g, placeholder);
  const escaped = marked.replace(/[.*+?^$()|[\]\\{}]/g, "\\$&");
  return new RegExp(`(?:^|> )${escaped.replaceAll(placeholder, ".+")}$`);
}

const SKIP_DIRS = new Set(["node_modules", "target", "dist", "gen", ".git"]);

function walk(dir: string, accept: (path: string) => boolean): string[] {
  return readdirSync(dir).flatMap((name) => {
    if (SKIP_DIRS.has(name)) return [];
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return walk(path, accept);
    return accept(path) ? [path] : [];
  });
}

const isTestFile = (path: string) =>
  /[\\/](tests?)[\\/]/.test(path) || /\.test\.tsx?$/.test(path);

/** Files whose header says `trace: ignore-ids` contain example or fixture IDs, not citations. */
export const IGNORE_MARKER = "trace: ignore-ids";
const ignoresIds = (text: string) => headerComment(text.split(/\r?\n/)).includes(IGNORE_MARKER);

export interface ScanResult {
  readonly tests: TestRef[];
  readonly modules: ModuleRef[];
}

/** Scans the Rust crates, the app crate, the UI and the CI tools. */
export function scanRepository(repoRoot: string): ScanResult {
  const rel = (path: string) => relative(repoRoot, path).replaceAll("\\", "/");
  const tests: TestRef[] = [];
  const modules: ModuleRef[] = [];

  const rustRoots: [string, string][] = [
    ...readdirSync(join(repoRoot, "crates")).map((c): [string, string] => [join(repoRoot, "crates", c), c]),
    [join(repoRoot, "src-tauri"), "caelum-desktop"],
  ];
  for (const [root, crate] of rustRoots) {
    for (const path of walk(root, (p) => p.endsWith(".rs"))) {
      const text = readFileSync(path, "utf8");
      // Marked files cite nothing; their fixtures could look like tests or IDs, so skip them.
      if (ignoresIds(text)) continue;
      tests.push(...scanRust(rel(path), crate, text));
      if (!isTestFile(path)) modules.push({ file: rel(path), ids: extractIds(text) });
    }
  }

  const tsRoots: [string, string, Runner][] = [
    [join(repoRoot, "ui"), "ui", "vitest"],
    [join(repoRoot, "tools", "ci"), "tools/ci", "node"],
  ];
  for (const [root, unit, runner] of tsRoots) {
    for (const path of walk(root, (p) => /\.(tsx?|css)$/.test(p) && !p.endsWith("bindings.ts"))) {
      const text = readFileSync(path, "utf8");
      if (ignoresIds(text)) continue;
      if (isTestFile(path)) tests.push(...scanTs(rel(path), unit, runner, text));
      else modules.push({ file: rel(path), ids: extractIds(text) });
    }
  }
  return { tests, modules: modules.filter((m) => m.ids.length > 0) };
}
