// Consistency of the supply-chain configuration (docs/09_VV_PLAN.md §4, CLAUDE.md stack):
// every accepted advisory is ignored identically by cargo-deny and cargo-audit and is recorded
// in the SOUP register; licence allow-lists for Rust and npm are the same.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { ALLOWED_LICENSES } from "../src/npm.ts";
import { REPO_ROOT } from "../src/repo.ts";

const read = (...parts: string[]) => readFileSync(join(REPO_ROOT, ...parts), "utf8");
const ids = (text: string) => [...new Set(text.match(/RUSTSEC-\d{4}-\d{4}/g) ?? [])].sort();

function tomlArray(text: string, key: string): string[] {
  const match = new RegExp(`^${key}\\s*=\\s*\\[([\\s\\S]*?)^\\]`, "m").exec(text);
  return [...(match?.[1] ?? "").matchAll(/^\s*"([^"]+)"/gm)].map((m) => m[1] ?? "");
}

describe("advisory ignores", () => {
  const deny = ids(read("deny.toml"));
  const audit = ids(read(".cargo", "audit.toml"));
  const register = read("docs", "third-party-risk.md");

  it("cargo-deny and cargo-audit ignore exactly the same advisories", () => {
    assert.deepEqual(deny, audit);
  });

  it("every ignored advisory has a row in the SOUP register", () => {
    for (const id of deny) assert.match(register, new RegExp(`^\\| ${id} \\|`, "m"), id);
  });

  it("every ignore in deny.toml states a reason", () => {
    const entries = read("deny.toml").match(/\{ id = "RUSTSEC-[^}]+\}/g) ?? [];
    assert.equal(entries.length, deny.length);
    for (const e of entries) assert.match(e, /reason = "[^"]{20,}"/, e);
  });
});

describe("licence allow-lists", () => {
  it("deny.toml (Rust) and the npm guard allow the same licences", () => {
    assert.deepEqual(tomlArray(read("deny.toml"), "allow").sort(), [...ALLOWED_LICENSES].sort());
  });

  it("allows no copyleft licence globally", () => {
    for (const licence of ALLOWED_LICENSES) assert.doesNotMatch(licence, /GPL|MPL|EPL|CDDL|EUPL|OSL/, licence);
  });
});
