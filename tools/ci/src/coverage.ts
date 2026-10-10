// Coverage gates (CLAUDE.md stack, docs/09 §4): caelum-alarms ≥ 95 % region coverage, the other core
// crates ≥ 85 % line coverage, measured over the whole workspace test run (a crate's code is often
// exercised by another crate's tests) and aggregated per crate from the llvm-cov JSON summary.
//
// A gate applies once the crate's milestone has started (it has code). Hardware transport files
// that only run against real devices are listed in EXCLUDED with the reason; they are verified on
// the hardware-in-the-loop bench (docs/09 §1), and the policy logic they use is unit-tested.

export interface Gate {
  readonly crate: string;
  readonly metric: "regions" | "lines";
  readonly min: number;
}

export const GATES: readonly Gate[] = [
  { crate: "caelum-alarms", metric: "regions", min: 95 },
  { crate: "caelum-domain", metric: "lines", min: 85 },
  { crate: "caelum-store", metric: "lines", min: 85 },
  { crate: "caelum-ingest", metric: "lines", min: 85 },
  { crate: "caelum-security", metric: "lines", min: 85 },
  { crate: "caelum-devices", metric: "lines", min: 85 },
  { crate: "caelum-core", metric: "lines", min: 85 },
];

/** Files outside the line gate, with the reason (reviewed like any other exception). */
export const EXCLUDED: readonly { file: string; reason: string }[] = [
  { file: "crates/caelum-devices/src/ble.rs", reason: "btleplug radio transport; HIL bench (policy: link.rs, measurement_characteristics)" },
  { file: "crates/caelum-devices/src/pairing.rs", reason: "OS pairing (WinRT, BlueZ); HIL bench (outcomes and addresses unit-tested)" },
  { file: "crates/caelum-devices/src/hid.rs", reason: "HID device I/O; HIL bench" },
];

interface Counts {
  readonly count: number;
  readonly covered: number;
}

export interface FileSummary {
  readonly filename: string;
  readonly lines: Counts;
  readonly regions: Counts;
}

/** The per-file summaries of `cargo llvm-cov --json --summary-only`. */
export function parseSummary(json: string): FileSummary[] {
  const data = JSON.parse(json) as { data: { files: { filename: string; summary: { lines: Counts; regions: Counts } }[] }[] };
  return data.data.flatMap((d) =>
    d.files.map((f) => ({ filename: f.filename.replaceAll("\\", "/"), lines: f.summary.lines, regions: f.summary.regions })),
  );
}

function crateOf(filename: string): string | undefined {
  return /\/crates\/(caelum-[a-z-]+)\/src\//.exec(filename)?.[1];
}

export interface GateResult {
  readonly gate: Gate;
  readonly percent: number | undefined;
  readonly ok: boolean;
}

/** Evaluates every gate. A crate with no measured code (not started) passes and is reported. */
export function evaluate(files: readonly FileSummary[], gates: readonly Gate[] = GATES): GateResult[] {
  return gates.map((gate) => {
    let count = 0;
    let covered = 0;
    for (const f of files) {
      if (crateOf(f.filename) !== gate.crate) continue;
      if (EXCLUDED.some((e) => f.filename.endsWith(e.file))) continue;
      count += f[gate.metric].count;
      covered += f[gate.metric].covered;
    }
    const percent = count === 0 ? undefined : (covered / count) * 100;
    return { gate, percent, ok: percent === undefined || percent >= gate.min };
  });
}
