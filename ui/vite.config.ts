import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri loads the dev server from a fixed port (src-tauri/tauri.conf.json `devUrl`).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_ENV_"],
  server: { host: "localhost", port: 5173, strictPort: true },
  build: {
    // WebView2 (Windows) is Chromium-based; WebKitGTK and WKWebView need Safari-level targets.
    target: ["chrome105", "safari15"],
    // Never inline assets as data: URLs; the release CSP allows fonts only from 'self'.
    assetsInlineLimit: 0,
    sourcemap: false,
  },
  test: {
    environment: "jsdom",
    include: ["tests/**/*.test.{ts,tsx}"],
    setupFiles: ["tests/setup.ts"],
  },
});
