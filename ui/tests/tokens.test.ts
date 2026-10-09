// @vitest-environment node
// Token rules from docs/07_UI_SPEC.md §1 (CR-002 D5, CR-003 §1) and CLAUDE.md rule 8.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { THEMES } from "../src/styles/contrast-pairs";
import { TOKENS_CSS, themeTokens } from "./tokens";

const INDEX_CSS = readFileSync(
  fileURLToPath(new URL("../src/styles/index.css", import.meta.url)),
  "utf8",
);

describe("tokens.css", () => {
  it("defines the light, dark and high-contrast themes", () => {
    for (const theme of THEMES) {
      expect(themeTokens(theme).get("canvas")).toMatch(/^#[0-9A-F]{6}$/i);
    }
  });

  it("uses Outfit only: no 'Google Sans' in any font stack", () => {
    const withoutComments = TOKENS_CSS.replace(/\/\*[\s\S]*?\*\//g, "");
    expect(withoutComments).not.toMatch(/Google Sans/i);
    expect(TOKENS_CSS).toMatch(/--font-sans:\s*'Outfit'/);
    expect(TOKENS_CSS).toMatch(/--font-dot:\s*'Outfit'/);
  });

  it("keeps scrollbars visible and the page scrollable", () => {
    expect(TOKENS_CSS).not.toMatch(/scrollbar-width:\s*none/);
    expect(TOKENS_CSS).not.toMatch(/::-webkit-scrollbar\s*\{[^}]*display:\s*none/);
    expect(TOKENS_CSS).not.toMatch(/body\s*\{[^}]*overflow:\s*hidden/);
  });

  it("defines the DRAFT alarm tokens in every theme and marks them DRAFT", () => {
    expect(TOKENS_CSS).toMatch(/DRAFT alarm presentation tokens/);
    for (const theme of THEMES) {
      const tokens = themeTokens(theme);
      for (const name of ["alarm-high", "alarm-medium", "on-alarm", "warning-ink", "on-warning"]) {
        expect(tokens.get(name), `${theme}: --${name}`).toMatch(/^#[0-9A-F]{6}$/i);
      }
    }
  });

  it("starts the light alarm tokens from the CR-002 values", () => {
    const light = themeTokens("light");
    expect(light.get("alarm-high")).toBe("#B42318");
    expect(light.get("alarm-medium")).toBe("#B54708");
  });

  it("keeps the prototype palette apart from the documented fixes", () => {
    const light = themeTokens("light");
    expect(light.get("canvas")).toBe("#D6E8E2");
    expect(light.get("signal")).toBe("#006D58");
    expect(light.get("warning")).toBe("#D4A017");
    expect(light.get("ink-3")).not.toBe("#58736B");
  });
});

describe("fonts (CLAUDE.md rule 8: never fetched at runtime)", () => {
  it("imports Outfit from the bundled @fontsource package only", () => {
    expect(INDEX_CSS).toMatch(/@import '@fontsource\/outfit\/400\.css'/);
    expect(INDEX_CSS).not.toMatch(/https?:\/\//);
  });
});
