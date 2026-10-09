// Offline tool cache (R-OPS-03, F-SYS-06, docs/OFFLINE_BUILD.md): manifest, tamper detection,
// verified downloads and the offline cargo configuration.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, describe, it } from "node:test";

import {
  cacheDir,
  hashFile,
  offlineCargoConfig,
  parseManifest,
  verifyCache,
  writeManifest,
} from "../src/offline.ts";

const roots: string[] = [];
function fakeCache(): string {
  const root = mkdtempSync(join(tmpdir(), "caelum-cache-"));
  roots.push(root);
  mkdirSync(join(root, "vendor", "itoa"), { recursive: true });
  writeFileSync(join(root, "vendor", "itoa", "lib.rs"), "pub fn f() {}\n");
  writeFileSync(join(root, "ITEMS.json"), "[]\n");
  return root;
}
after(() => {
  for (const r of roots) rmSync(r, { recursive: true, force: true });
});

describe("manifest", () => {
  it("lists every file with its SHA-256 in sha256sum format", async () => {
    const cache = fakeCache();
    assert.equal(await writeManifest(cache), 2);
    const entries = parseManifest((await import("node:fs")).readFileSync(join(cache, "MANIFEST.sha256"), "utf8"));
    assert.deepEqual([...entries.keys()], ["ITEMS.json", "vendor/itoa/lib.rs"]);
    assert.equal(entries.get("ITEMS.json"), await hashFile(join(cache, "ITEMS.json")));
  });

  it("an intact cache verifies", async () => {
    const cache = fakeCache();
    await writeManifest(cache);
    assert.deepEqual(await verifyCache(cache), []);
  });

  it("detects a changed file", async () => {
    const cache = fakeCache();
    await writeManifest(cache);
    writeFileSync(join(cache, "vendor", "itoa", "lib.rs"), "pub fn f() { /* tampered */ }\n");
    assert.deepEqual(await verifyCache(cache), ["checksum mismatch: vendor/itoa/lib.rs"]);
  });

  it("detects a removed file and an added file", async () => {
    const cache = fakeCache();
    await writeManifest(cache);
    unlinkSync(join(cache, "ITEMS.json"));
    writeFileSync(join(cache, "vendor", "extra.rs"), "// planted\n");
    assert.deepEqual((await verifyCache(cache)).sort(), ["missing: ITEMS.json", "not in manifest: vendor/extra.rs"]);
  });

  it("refuses a cache without a manifest", async () => {
    assert.match((await verifyCache(fakeCache()))[0] ?? "", /MANIFEST\.sha256 not found/);
  });

  it("refuses links: they can dangle or point outside the cache", async () => {
    const cache = fakeCache();
    await writeManifest(cache);
    const outside = mkdtempSync(join(tmpdir(), "caelum-outside-"));
    roots.push(outside);
    symlinkSync(outside, join(cache, "vendor", "escape"), "junction");
    assert.deepEqual(await verifyCache(cache), ["link not allowed: vendor/escape"]);
    await assert.rejects(writeManifest(cache), /links are not allowed/);
  });

  it("rejects malformed manifest lines", () => {
    assert.throws(() => parseManifest("not-a-hash  file\n"), /malformed/);
  });
});

describe("settings", () => {
  it("builds cargo config that uses only the vendored crates, offline", () => {
    const config = offlineCargoConfig("D:\\cache\\vendor");
    assert.match(config, /replace-with = "vendored-sources"/);
    assert.match(config, /directory = "D:\/cache\/vendor"/);
    assert.match(config, /\[net\]\noffline = true/);
  });

  it("takes the cache from --cache or CAELUM_TOOL_CACHE and refuses neither", () => {
    assert.equal(cacheDir(["--cache", "D:/cache"]), "D:/cache");
    const saved = process.env["CAELUM_TOOL_CACHE"];
    delete process.env["CAELUM_TOOL_CACHE"];
    assert.throws(() => cacheDir([]), /CAELUM_TOOL_CACHE/);
    if (saved !== undefined) process.env["CAELUM_TOOL_CACHE"] = saved;
  });
});
