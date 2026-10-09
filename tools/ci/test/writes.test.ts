// Self-tests for the write-exception guard (CLAUDE.md rule 5, R-SEC-07, SR-011).
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { REPO_ROOT } from "../src/repo.ts";
import {
  checkWrites,
  parseRegister,
  readRegister,
  readSources,
  type RegisterEntry,
  type SourceFile,
  VEHICLE_ONLY_RULES,
  WRITE_RULES,
} from "../src/writes.ts";

const devices = (text: string): SourceFile => ({ crate: "caelum-devices", path: "devices.rs", text });
const vehicle = (text: string): SourceFile => ({ crate: "caelum-vehicle", path: "vehicle.rs", text });

const entry = (overrides: Partial<RegisterEntry> = {}): RegisterEntry => ({
  id: "WX-001",
  crate: "caelum-devices",
  category: "a",
  reviewer: "A. Reviewer",
  ...overrides,
});

/** One example call per rule; every one must be caught. */
const EXAMPLES: Record<string, string> = {
  "io-write": "port.write(&request)?;",
  "io-write-all": "port.write_all(&request)?;",
  "io-write-vectored": "port.write_vectored(&bufs)?;",
  "io-write-trait": "use std::io::Write;",
  "ble-write-type": "p.write(&c, &data, WriteType::WithResponse).await?;",
  "ble-write-descriptor": "p.write_descriptor(&d, &data).await?;",
  "ble-write-ext": "c.write_ext(&data, &opts).await?;",
  "ble-subscribe": "p.subscribe(&characteristic).await?;",
  "hid-report": "dev.send_feature_report(&report)?;",
  "socket-send-to": "socket.send_to(&packet, addr)?;",
  "vehicle-send": "socket.send(&packet)?;",
  "vehicle-connect": "socket.connect(addr)?;",
};

describe("write rules", () => {
  it("has an example for every rule", () => {
    const ids = [...WRITE_RULES, ...VEHICLE_ONLY_RULES].map((r) => r.id).sort();
    assert.deepEqual(ids, Object.keys(EXAMPLES).sort());
  });

  for (const rule of WRITE_RULES) {
    it(`catches ${rule.id} in caelum-devices without a marker`, () => {
      const findings = checkWrites([devices(EXAMPLES[rule.id] ?? "")], []);
      const own = findings.filter((f) => f.message.includes(`[${rule.id}]`));
      assert.equal(own.length, 1, JSON.stringify(findings));
      assert.match(own[0]?.message ?? "", /without a WRITE-EXCEPTION marker/);
    });
  }

  for (const rule of [...WRITE_RULES, ...VEHICLE_ONLY_RULES]) {
    it(`rejects ${rule.id} in caelum-vehicle even with a marker`, () => {
      const text = `// WRITE-EXCEPTION: WX-001\n${EXAMPLES[rule.id] ?? ""}`;
      const findings = checkWrites([vehicle(text)], [entry()]);
      assert.ok(findings.some((f) => /caelum-vehicle has no write API/.test(f.message)), JSON.stringify(findings));
    });
  }

  it("does not flag reads, lock guards, channel subscriptions or comments in caelum-devices", () => {
    const text = [
      "let value = p.read(&characteristic).await?;",
      "let guard = state.write();",
      "let rx = events.subscribe();",
      "tx.send(reading)?; // internal channel, not a device write",
      "// p.write(&c, &data, WriteType::WithResponse) is not allowed here",
      "/// Never call `.write_all(` on the port.",
      "use std::fmt::Write as _;",
    ].join("\n");
    assert.deepEqual(checkWrites([devices(text)], []), []);
  });
});

describe("markers and register", () => {
  it("accepts a write with a marker on the line above that matches a reviewed entry", () => {
    const text = "// WRITE-EXCEPTION: WX-001\np.subscribe(&characteristic).await?;";
    assert.deepEqual(checkWrites([devices(text)], [entry()]), []);
  });

  it("accepts a marker on the same line", () => {
    const text = "p.subscribe(&c).await?; // WRITE-EXCEPTION: WX-001";
    assert.deepEqual(checkWrites([devices(text)], [entry()]), []);
  });

  it("rejects a marker whose ID is not registered", () => {
    const findings = checkWrites([devices("// WRITE-EXCEPTION: WX-009\np.subscribe(&c).await?;")], []);
    assert.match(findings[0]?.message ?? "", /WX-009 is not in docs\/write-exceptions.md/);
  });

  it("rejects an entry without a reviewer", () => {
    const text = "// WRITE-EXCEPTION: WX-001\np.subscribe(&c).await?;";
    const findings = checkWrites([devices(text)], [entry({ reviewer: "" })]);
    assert.ok(findings.some((f) => /no reviewer recorded/.test(f.message)));
  });

  it("rejects an entry with a category outside a, b, c", () => {
    const text = "// WRITE-EXCEPTION: WX-001\np.subscribe(&c).await?;";
    const findings = checkWrites([devices(text)], [entry({ category: "d" })]);
    assert.ok(findings.some((f) => /category must be a, b or c/.test(f.message)));
  });

  it("rejects any register entry for caelum-vehicle", () => {
    const findings = checkWrites([], [entry({ crate: "caelum-vehicle" })]);
    assert.ok(findings.some((f) => /caelum-vehicle has no write API/.test(f.message)));
  });

  it("rejects a stale entry with no call site", () => {
    const findings = checkWrites([devices("let x = 1;")], [entry()]);
    assert.ok(findings.some((f) => /stale entry/.test(f.message)));
  });

  it("parses the register table", () => {
    const md = [
      "## Register",
      "| ID | Crate | Call | Category | Ref | Reason | Reviewer | Date |",
      "|---|---|---|---|---|---|---|---|",
      "| WX-001 | caelum-devices | ble::subscribe | A | HRS 0x2902 | notifications | B. Smith | 2026-10-10 |",
    ].join("\n");
    assert.deepEqual(parseRegister(md), [
      { id: "WX-001", crate: "caelum-devices", category: "a", reviewer: "B. Smith" },
    ]);
  });
});

describe("repository", () => {
  it("has an empty register (M0 ships with zero exceptions)", () => {
    assert.deepEqual(readRegister(REPO_ROOT), []);
  });

  it("has no unregistered writes in caelum-devices or caelum-vehicle", () => {
    const files = readSources(REPO_ROOT);
    assert.ok(files.length >= 2, "both crates must be scanned");
    assert.deepEqual(checkWrites(files, readRegister(REPO_ROOT)), []);
  });
});
