import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { playwright } from "@vitest/browser-playwright";
import { defineConfig } from "vitest/config";

// M5 gate: axe accessibility checks in Vitest browser mode (Chromium via Playwright), with the
// real stylesheet so colour contrast is measured as rendered. Screenshots for review are written
// next to the tests (__screenshots__/). Standalone (not merged with vite.config.ts): merging would
// concatenate the include/exclude lists and pull the jsdom/node suites into the browser.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  // Pre-bundled so Vite does not reload the page mid-run when it discovers them.
  optimizeDeps: { include: ["axe-core", "react", "react-dom/client", "@testing-library/react", "@tauri-apps/api/core", "@tauri-apps/api/event"] },
  test: {
    include: ["tests/a11y/**/*.browser.test.tsx"],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: "chromium" }],
      viewport: { width: 1280, height: 900 },
    },
  },
});
