// Reads ui/src/styles/tokens.css and resolves the colour tokens of each theme, the way the
// browser does: light is `:root`; dark and high contrast override it.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import type { Theme } from "../src/styles/contrast-pairs";

export const TOKENS_CSS = readFileSync(
  fileURLToPath(new URL("../src/styles/tokens.css", import.meta.url)),
  "utf8",
);

const THEME_SELECTORS: Record<Theme, string> = {
  light: ":root",
  dark: ':root[data-theme="dark"]',
  "high-contrast": ':root[data-theme="high-contrast"]',
};

function block(css: string, selector: string): string {
  const start = css.indexOf(`${selector} {`);
  if (start < 0) throw new Error(`tokens.css has no block for ${selector}`);
  const end = css.indexOf("}", start);
  return css.slice(start, end);
}

function declarations(text: string): Map<string, string> {
  const result = new Map<string, string>();
  for (const match of text.matchAll(/--([\w-]+)\s*:\s*([^;]+);/g)) {
    const [, name, value] = match;
    if (name !== undefined && value !== undefined) result.set(name, value.trim());
  }
  return result;
}

/** All custom properties for a theme, with `var(--x)` references resolved. */
export function themeTokens(theme: Theme): Map<string, string> {
  const tokens = declarations(block(TOKENS_CSS, THEME_SELECTORS.light));
  if (theme !== "light") {
    for (const [name, value] of declarations(block(TOKENS_CSS, THEME_SELECTORS[theme]))) {
      tokens.set(name, value);
    }
  }
  const resolve = (value: string, depth = 0): string => {
    const ref = /^var\(--([\w-]+)\)$/.exec(value);
    if (ref?.[1] === undefined) return value;
    const target = tokens.get(ref[1]);
    if (target === undefined || depth > 8) throw new Error(`unresolved ${value}`);
    return resolve(target, depth + 1);
  };
  return new Map([...tokens].map(([name, value]) => [name, resolve(value)]));
}

/** sRGB channel to linear light (WCAG 2.x). */
function linear(channel: number): number {
  const c = channel / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

export function luminance(hex: string): number {
  const match = /^#([0-9a-f]{6})$/i.exec(hex);
  if (match?.[1] === undefined) throw new Error(`not a #RRGGBB colour: ${hex}`);
  const n = Number.parseInt(match[1], 16);
  return 0.2126 * linear((n >> 16) & 0xff) + 0.7152 * linear((n >> 8) & 0xff) + 0.0722 * linear(n & 0xff);
}

/** WCAG 2.x contrast ratio between two opaque colours. */
export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}
