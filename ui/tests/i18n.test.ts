// M5 gate (F-SYS-11): no user-visible string outside the localisation catalog. Every .tsx file in
// src/ is parsed; JSX text and the text attributes users see or hear (aria-label, title, alt,
// placeholder, label) may not contain letters as literals: they must come from src/strings.ts.
// @vitest-environment node
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import ts from "typescript";
import { describe, expect, it } from "vitest";

const SRC = fileURLToPath(new URL("../src", import.meta.url));
const TEXT_ATTRIBUTES = new Set(["aria-label", "title", "alt", "placeholder", "label", "aria-description"]);
const LETTERS = /\p{L}/u;

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return files(path);
    return path.endsWith(".tsx") ? [path] : [];
  });
}

export function literals(path: string, text: string): string[] {
  const source = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const found: string[] = [];
  const visit = (node: ts.Node) => {
    if (ts.isJsxText(node) && LETTERS.test(node.getText())) {
      found.push(`text "${node.getText().trim()}"`);
    }
    if (ts.isJsxAttribute(node) && TEXT_ATTRIBUTES.has(node.name.getText())) {
      const init = node.initializer;
      const literal =
        init !== undefined &&
        (ts.isStringLiteral(init) ||
          (ts.isJsxExpression(init) && init.expression !== undefined && (ts.isStringLiteral(init.expression) || ts.isNoSubstitutionTemplateLiteral(init.expression))));
      if (literal && LETTERS.test(init.getText())) found.push(`${node.name.getText()}=${init.getText()}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return found.map((f) => `${relative(SRC, path)}: ${f}`);
}

describe("localisation catalog", () => {
  it("has no user-visible literal text outside src/strings.ts", () => {
    const problems = files(SRC).flatMap((p) => literals(p, readFileSync(p, "utf8")));
    expect(problems).toEqual([]);
  });

  it("catches literal text and labels (self-test)", () => {
    const bad = literals(join(SRC, "x.tsx"), `export const A = () => <div aria-label="Close">Hello <b title={"Tip"}>{"ok"}</b>!</div>;`);
    expect(bad).toEqual(['x.tsx: aria-label="Close"', 'x.tsx: text "Hello"', 'x.tsx: title={"Tip"}']);
    expect(literals(join(SRC, "y.tsx"), `export const B = () => <span aria-hidden="true">—</span>;`)).toEqual([]);
  });
});
