// WebView2 Fixed Version runtime (CR-004, R-OPS-03): only a validly signed Microsoft package is
// accepted, and the pinned version is consistent with tauri.conf.json.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { REPO_ROOT } from "../src/repo.ts";
import { RELEASE_CONFIG, signatureProblem, WEBVIEW2 } from "../src/webview2.ts";

const microsoft = {
  status: "Valid",
  subject: "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US",
  issuer: "CN=Microsoft Code Signing PCA 2024, O=Microsoft Corporation, C=US",
};

describe("WebView2 package signature", () => {
  it("accepts a valid Microsoft Corporation signature", () => {
    assert.equal(signatureProblem(microsoft), undefined);
  });

  it("rejects a tampered, unsigned or foreign-signed package", () => {
    assert.match(signatureProblem({ ...microsoft, status: "HashMismatch" }) ?? "", /HashMismatch/);
    assert.match(signatureProblem({ ...microsoft, status: "NotSigned" }) ?? "", /NotSigned/);
    assert.match(signatureProblem({ ...microsoft, subject: "CN=Contoso Ltd, O=Contoso" }) ?? "", /unexpected signer/);
    assert.match(signatureProblem({ ...microsoft, issuer: "CN=Some CA, O=Other" }) ?? "", /unexpected issuer/);
  });

  it("reports an unreadable signature with its reason instead of a blank status", () => {
    const problem = signatureProblem({ status: "", subject: "", issuer: "", error: "Cannot find path" });
    assert.equal(problem, "signature could not be read: Cannot find path");
    assert.equal(signatureProblem({ status: "", subject: "", issuer: "" }), "Authenticode status (none)");
  });
});

describe("pinned runtime", () => {
  it("names one version consistently in URL, cab and folder", () => {
    for (const s of [WEBVIEW2.url, WEBVIEW2.cab, WEBVIEW2.folder]) assert.ok(s.includes(WEBVIEW2.version), s);
    assert.match(WEBVIEW2.url, /^https:\/\/msedge\.sf\.dl\.delivery\.mp\.microsoft\.com\//);
  });

  it("the release overlay uses fixedRuntime from src-tauri/webview2-fixed-runtime", () => {
    const conf = JSON.parse(readFileSync(join(REPO_ROOT, RELEASE_CONFIG), "utf8")) as {
      bundle: { windows: { webviewInstallMode: { type: string; path: string } } };
    };
    assert.deepEqual(conf.bundle.windows.webviewInstallMode, { type: "fixedRuntime", path: "./webview2-fixed-runtime/" });
  });

  it("every release build script passes the release overlay", () => {
    for (const script of ["tools/offline/build-offline.ts", "tools/offline/prepare.ts"]) {
      const text = readFileSync(join(REPO_ROOT, script), "utf8");
      // Only real command invocations (sh(`...`)), not mentions in comments.
      const builds = text.match(/sh\(`pnpm tauri build[^`]*`/g) ?? [];
      assert.ok(builds.length > 0, `${script} runs no tauri build`);
      for (const b of builds) assert.match(b, /--config \$\{RELEASE_CONFIG\}/, `${script}: ${b}`);
    }
  });
});
