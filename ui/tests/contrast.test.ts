// @vitest-environment node
// Contrast contract (docs/07_UI_SPEC.md §1, CR-002 D5, F-SYS-10, R-ALM-02): every declared token
// pair meets its threshold in every theme. Text >= 4.5:1, UI elements >= 3:1.
import { describe, expect, it } from "vitest";

import { CONTRAST_PAIRS, THEMES } from "../src/styles/contrast-pairs";
import { contrast, themeTokens } from "./tokens";

const THRESHOLD = { text: 4.5, ui: 3 } as const;

describe("contrast helper", () => {
  it("matches the WCAG reference values", () => {
    expect(contrast("#000000", "#FFFFFF")).toBeCloseTo(21, 5);
    expect(contrast("#FFFFFF", "#FFFFFF")).toBeCloseTo(1, 5);
    // The prototype's amber on white, the reason CR-002 D5 exists.
    expect(contrast("#D4A017", "#FFFFFF")).toBeLessThan(3);
  });

  it("rejects colours it cannot measure instead of guessing", () => {
    expect(() => contrast("rgba(0,0,0,.5)", "#FFFFFF")).toThrow();
  });
});

describe.each(THEMES)("%s theme", (theme) => {
  const tokens = themeTokens(theme);

  it.each(CONTRAST_PAIRS.map((p) => [`${p.fg} on ${p.bg} (${p.use})`, p] as const))(
    "%s meets its threshold",
    (_label, pair) => {
      const fg = tokens.get(pair.fg);
      const bg = tokens.get(pair.bg);
      expect(fg, `--${pair.fg} is not defined`).toBeDefined();
      expect(bg, `--${pair.bg} is not defined`).toBeDefined();
      const ratio = contrast(fg ?? "", bg ?? "");
      expect(
        ratio,
        `--${pair.fg} ${fg ?? ""} on --${pair.bg} ${bg ?? ""} is ${ratio.toFixed(2)}:1`,
      ).toBeGreaterThanOrEqual(THRESHOLD[pair.use]);
    },
  );
});

describe("pair contract", () => {
  it("never uses --warning as a text colour (fill only, CR-002 D5)", () => {
    const misuse = CONTRAST_PAIRS.filter((p) => p.use === "text" && p.fg === "warning");
    expect(misuse).toEqual([]);
  });
});
