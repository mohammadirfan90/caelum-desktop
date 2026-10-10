// Self-tests for the coverage gate checker (docs/09 §4).
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { evaluate, type FileSummary, GATES, parseSummary } from "../src/coverage.ts";

const file = (filename: string, lines: [number, number], regions: [number, number]): FileSummary => ({
  filename,
  lines: { count: lines[0], covered: lines[1] },
  regions: { count: regions[0], covered: regions[1] },
});

describe("coverage gates", () => {
  it("aggregates per crate and fails a crate under its gate", () => {
    const files = [
      file("G:/repo/crates/caelum-alarms/src/engine.rs", [100, 99], [100, 96]),
      file("G:/repo/crates/caelum-alarms/src/ruleset.rs", [100, 99], [100, 93]),
      file("/r/crates/caelum-store/src/db.rs", [100, 80], [100, 80]),
    ];
    const r = evaluate(files);
    const alarms = r.find((x) => x.gate.crate === "caelum-alarms");
    assert.equal(alarms?.percent, 94.5);
    assert.equal(alarms?.ok, false, "regions are what counts for caelum-alarms");
    assert.equal(r.find((x) => x.gate.crate === "caelum-store")?.ok, false);
  });

  it("passes crates with no code yet and skips excluded hardware files", () => {
    const files = [
      file("/r/crates/caelum-devices/src/ble.rs", [100, 0], [100, 0]),
      file("/r/crates/caelum-devices/src/link.rs", [100, 90], [100, 90]),
    ];
    const r = evaluate(files);
    assert.equal(r.find((x) => x.gate.crate === "caelum-devices")?.percent, 90);
    const core = r.find((x) => x.gate.crate === "caelum-core");
    assert.equal(core?.percent, undefined);
    assert.equal(core?.ok, true);
  });

  it("reads the llvm-cov JSON summary with Windows paths", () => {
    const json = JSON.stringify({
      data: [{ files: [{ filename: "G:\\repo\\crates\\caelum-ingest\\src\\lib.rs", summary: { lines: { count: 4, covered: 4 }, regions: { count: 2, covered: 2 } } }] }],
    });
    const files = parseSummary(json);
    assert.equal(files[0]?.filename, "G:/repo/crates/caelum-ingest/src/lib.rs");
    assert.equal(evaluate(files).find((x) => x.gate.crate === "caelum-ingest")?.percent, 100);
  });

  it("gates caelum-alarms on regions at 95 % and every other crate on lines at 85 %", () => {
    for (const g of GATES) {
      assert.deepEqual([g.metric, g.min], g.crate === "caelum-alarms" ? ["regions", 95] : ["lines", 85]);
    }
  });
});
