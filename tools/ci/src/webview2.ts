// WebView2 Fixed Version runtime (CR-004 option A): a pinned, Microsoft-signed browser engine that
// is shipped inside CAELUM, so Windows release builds need no network (R-OPS-03) and every laptop
// renders the safety UI with the same, tested engine.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, renameSync, rmSync } from "node:fs";
import { join } from "node:path";

import { REPO_ROOT } from "./repo.ts";

/** Pinned runtime. Change only as a reviewed update (docs/third-party-risk.md). */
export const WEBVIEW2 = {
  version: "155.0.4283.45",
  arch: "x64",
  url: "https://msedge.sf.dl.delivery.mp.microsoft.com/filestreamingservice/files/01b0fbd9-6ea3-4e12-818a-a71f73f3855b/Microsoft.WebView2.FixedVersionRuntime.155.0.4283.45.x64.cab",
  cab: "Microsoft.WebView2.FixedVersionRuntime.155.0.4283.45.x64.cab",
  /** Folder name inside the cab. */
  folder: "Microsoft.WebView2.FixedVersionRuntime.155.0.4283.45.x64",
} as const;

/** Where tauri.release.conf.json expects the runtime (git-ignored, filled from the tool cache). */
export const RUNTIME_DIR = join(REPO_ROOT, "src-tauri", "webview2-fixed-runtime");

/**
 * Release overlay for `tauri build --config`: switches WebView2 to the bundled fixed runtime.
 * tauri.conf.json itself uses `skip`, so development builds, tests and clippy need no runtime.
 * Every release build must pass this file.
 */
export const RELEASE_CONFIG = "src-tauri/tauri.release.conf.json";

export interface Signature {
  readonly status: string;
  readonly subject: string;
  readonly issuer: string;
}

/** The only acceptable signer of the runtime package. */
export function signatureProblem(sig: Signature): string | undefined {
  if (sig.status !== "Valid") return `Authenticode status ${sig.status}`;
  if (!/^CN=Microsoft Corporation,/.test(sig.subject)) return `unexpected signer ${sig.subject}`;
  if (!/O=Microsoft Corporation/.test(sig.issuer)) return `unexpected issuer ${sig.issuer}`;
  return undefined;
}

/** Reads the Authenticode signature of a file with PowerShell (Windows). */
export function authenticode(path: string): Signature {
  const script =
    "$s = Get-AuthenticodeSignature -LiteralPath $env:CAELUM_SIGNED_FILE; " +
    "[pscustomobject]@{ status = [string]$s.Status; subject = [string]$s.SignerCertificate.Subject; " +
    "issuer = [string]$s.SignerCertificate.Issuer } | ConvertTo-Json -Compress";
  const powershell = join(process.env["SystemRoot"] ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
  const r = spawnSync(powershell, ["-NoProfile", "-NonInteractive", "-Command", script], {
    encoding: "utf8",
    env: { ...process.env, CAELUM_SIGNED_FILE: path },
  });
  if (r.status !== 0) throw new Error(`Get-AuthenticodeSignature failed: ${r.stderr}`);
  return JSON.parse(r.stdout) as Signature;
}

/**
 * Expands the cached cab into src-tauri/webview2-fixed-runtime. The cab's integrity is already
 * proven by the tool-cache manifest; its signature is checked again here as defence in depth
 * (`requireValidSignature` is false on air-gapped machines, where revocation data may be missing:
 * a definite mismatch or a missing signature still fails).
 */
export function installRuntime(cabPath: string, requireValidSignature: boolean): Signature {
  if (process.platform !== "win32") throw new Error("the WebView2 runtime is only used for Windows builds");
  const sig = authenticode(cabPath);
  const problem = signatureProblem(sig);
  if (problem !== undefined && (requireValidSignature || /HashMismatch|NotSigned|unexpected/.test(problem))) {
    throw new Error(`${WEBVIEW2.cab}: ${problem}`);
  }
  const staging = `${RUNTIME_DIR}.staging`;
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(staging, { recursive: true });
  // Absolute path: Git Bash and similar shells put a GNU `expand` (a text tool) earlier on PATH.
  const expandExe = join(process.env["SystemRoot"] ?? "C:\\Windows", "System32", "expand.exe");
  const r = spawnSync(expandExe, [cabPath, "-F:*", staging], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`expand.exe failed: ${r.stdout}${r.stderr}`);
  const expanded = join(staging, WEBVIEW2.folder);
  if (!existsSync(join(expanded, "msedgewebview2.exe"))) {
    throw new Error(`${WEBVIEW2.cab} did not contain ${WEBVIEW2.folder}/msedgewebview2.exe (found: ${readdirSync(staging).join(", ")})`);
  }
  rmSync(RUNTIME_DIR, { recursive: true, force: true });
  renameSync(expanded, RUNTIME_DIR);
  rmSync(staging, { recursive: true, force: true });
  return sig;
}
