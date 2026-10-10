// End-to-end tests of the desktop app with tauri-driver + WebdriverIO (CLAUDE.md stack; M6 gate).
// Runs a `test-equipment` debug build (never a release build, CLAUDE.md rule 3) on its own data
// directory, with the emulated oximeter's scenario given by the test.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repo = resolve(import.meta.dirname, "..");
const exe = process.platform === "win32" ? "caelum-desktop.exe" : "caelum-desktop";
export const application = join(repo, "target", "debug", exe);

/** Data directory and scenario shared by the specs (set before the app starts). */
export const dataDir = mkdtempSync(join(tmpdir(), "caelum-e2e-"));
export const scenarioPath = join(dataDir, "oximeter.scenario");

// The emulated oximeter (CR-012 §8): 20 s at 97 %, then a desaturation to 82 % for 120 s.
const lines: string[] = ["# M6 end-to-end scenario: values come from this file only"];
for (let s = 0; s < 20; s += 1) lines.push(`${String(s)} plx 97 72`);
for (let s = 20; s < 140; s += 1) lines.push(`${String(s)} plx 82 110`);
writeFileSync(scenarioPath, `${lines.join("\n")}\n`);

let driver: ChildProcess | undefined;

export const config: WebdriverIO.Config = {
  runner: "local",
  specs: ["./specs/**/*.e2e.ts"],
  maxInstances: 1,
  hostname: "127.0.0.1",
  port: 4444,
  capabilities: [
    {
      // @ts-expect-error tauri-driver capability
      "tauri:options": { application },
    },
  ],
  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 600_000 },
  onPrepare: () => {
    process.env.CAELUM_TE_DATA_DIR = dataDir;
    process.env.CAELUM_TE_SCENARIO = scenarioPath;
  },
  beforeSession: () => {
    driver = spawn("tauri-driver", [], { stdio: [null, process.stdout, process.stderr] });
  },
  afterSession: () => {
    driver?.kill();
  },
};
