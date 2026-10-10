// Role-based navigation (ROLE_MATRIX §6, F-SYS-01) and station preferences (F-SYS-10: themes,
// text size to 200 %, glove mode, reduced motion).
// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { NAVIGATION, SCREENS, screensFor } from "../src/navigation";
import { applyPrefs, DEFAULT_PREFS, loadPrefs, savePrefs } from "../src/settings";

// jsdom has no file: import.meta.url; Vitest runs with ui/ as the working directory.
const MATRIX = readFileSync(resolve(process.cwd(), "../docs/ROLE_MATRIX.md"), "utf8");

const LABELS: Record<string, keyof typeof NAVIGATION> = {
  Astronaut: "astronaut",
  CMO: "cmo",
  "Flight Surgeon": "flight_surgeon",
  BME: "bme",
  MCC: "mcc",
  Admin: "administrator",
};

describe("navigation", () => {
  it("shows each role exactly the screens of ROLE_MATRIX §6", () => {
    const start = MATRIX.indexOf("## 6. Navigation");
    const rows = MATRIX.slice(start)
      .split("\n")
      .filter((l) => l.startsWith("|"))
      .map((l) => l.split("|").slice(1, -1).map((c) => c.trim()));
    const header = rows[0] ?? [];
    for (const row of rows.slice(2)) {
      const screen = (row[0] ?? "").toLowerCase().replace("environment", "env");
      expect(SCREENS as readonly string[]).toContain(screen);
      header.slice(1).forEach((label, i) => {
        const role = LABELS[label];
        expect(role, label).toBeDefined();
        if (role === undefined) return;
        const shown = (NAVIGATION[role] as readonly string[]).includes(screen);
        expect(shown, `${screen} for ${label}`).toBe(row[i + 1] === "yes");
      });
    }
    expect(screensFor("device_gateway")).toEqual([]);
  });
});

describe("preferences", () => {
  it("applies theme, text size, glove mode and reduced motion to the document root", () => {
    const root = document.createElement("html");
    applyPrefs({ theme: "high-contrast", textScale: 200, glove: true, reducedMotion: true }, root);
    expect(root.dataset.theme).toBe("high-contrast");
    expect(root.style.fontSize).toBe("200%");
    expect(root.dataset.glove).toBe("on");
    expect(root.dataset.motion).toBe("reduce");
  });

  it("round-trips through storage and falls back to defaults on bad or missing data", () => {
    window.localStorage.clear();
    expect(loadPrefs()).toEqual(DEFAULT_PREFS);
    savePrefs({ theme: "dark", textScale: 150, glove: false, reducedMotion: true });
    expect(loadPrefs()).toEqual({ theme: "dark", textScale: 150, glove: false, reducedMotion: true });
    window.localStorage.setItem("caelum.prefs", JSON.stringify({ theme: "neon", textScale: 300 }));
    expect(loadPrefs()).toEqual(DEFAULT_PREFS);
    window.localStorage.setItem("caelum.prefs", "{not json");
    expect(loadPrefs()).toEqual(DEFAULT_PREFS);
  });
});
