// trace: ignore-ids (this file documents the ID formats with examples; it cites none)
// Requirement IDs as written in the docs and in code (CLAUDE.md rule 9):
//   F-ACQ-01, F-ML-02, R-SEC-04, SR-012, N-01
// plus the shorthand forms the docs use for several IDs:
//   F-ACQ-01/02/04   SR-001..006   SR-001..SR-019   F-SIG-01..07   F-ACQ-01…05 (ellipsis)
// and the snake-case forms usable in Rust test names: f_alm_03, r_sec_04, sr_012.

const PREFIX = String.raw`(?:F-[A-Z]{2,4}-|R-[A-Z]{3}-|SR-|N-)`;
const ID_GROUP = new RegExp(
  String.raw`\b(${PREFIX})(\d{2,3})((?:/\d{2,3})*)(?:(?:\.\.|…)(?:${PREFIX})?(\d{2,3}))?`,
  "g",
);

function pad(n: number, width: number): string {
  return String(n).padStart(width, "0");
}

/** Every requirement ID mentioned in `text`, shorthand ranges and lists expanded, in order. */
export function extractIds(text: string): string[] {
  const out: string[] = [];
  for (const m of text.matchAll(ID_GROUP)) {
    const [, prefix = "", first = "", list = "", rangeEnd] = m;
    const width = first.length;
    if (rangeEnd !== undefined) {
      const start = Number(first);
      const end = Number(rangeEnd);
      if (end < start || end - start > 100) {
        out.push(`${prefix}${first}`);
        continue;
      }
      for (let n = start; n <= end; n += 1) out.push(`${prefix}${pad(n, width)}`);
      continue;
    }
    out.push(`${prefix}${first}`);
    for (const extra of list.split("/").filter((s) => s.length > 0)) out.push(`${prefix}${extra}`);
  }
  return [...new Set(out)];
}

/** IDs written in snake case in a test function name, e.g. `sr_012_clock_fault` → SR-012. */
export function idsFromName(name: string): string[] {
  const out: string[] = [];
  for (const m of name.matchAll(/(?:^|_)sr_(\d{3})(?=_|$)/g)) out.push(`SR-${m[1] ?? ""}`);
  for (const m of name.matchAll(/(?:^|_)([fr])_([a-z]{2,4})_(\d{2})(?=_|$)/g)) {
    out.push(`${(m[1] ?? "").toUpperCase()}-${(m[2] ?? "").toUpperCase()}-${m[3] ?? ""}`);
  }
  return [...new Set([...out, ...extractIds(name)])];
}
