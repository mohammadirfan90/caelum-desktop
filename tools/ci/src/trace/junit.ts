// trace: ignore-ids (this file documents the ID formats with examples; it cites none)
// Minimal JUnit XML reader for the three test runners' reports (nextest, Vitest, node:test).
export type Outcome = "passed" | "failed" | "skipped";

export interface CaseResult {
  /** Rust: crate or crate::test-binary. Vitest: file relative to ui/. node: file (set by caller). */
  readonly classname: string;
  readonly name: string;
  readonly outcome: Outcome;
}

const ENTITIES: Record<string, string> = { "&lt;": "<", "&gt;": ">", "&amp;": "&", "&quot;": '"', "&apos;": "'" };

function decode(text: string): string {
  return text
    .replace(/&#x([0-9a-f]+);/gi, (_, hex: string) => String.fromCodePoint(Number.parseInt(hex, 16)))
    .replace(/&#(\d+);/g, (_, dec: string) => String.fromCodePoint(Number(dec)))
    .replace(/&(?:lt|gt|amp|quot|apos);/g, (e) => ENTITIES[e] ?? e);
}

function attributes(text: string): Map<string, string> {
  return new Map([...text.matchAll(/([\w:-]+)="([^"]*)"/g)].map((m) => [m[1] ?? "", decode(m[2] ?? "")]));
}

export function parseJunit(xml: string): CaseResult[] {
  const cases: CaseResult[] = [];
  for (const m of xml.matchAll(/<testcase\b([^>]*?)(?:\/>|>([\s\S]*?)<\/testcase>)/g)) {
    const attrs = attributes(m[1] ?? "");
    const body = m[2] ?? "";
    const outcome: Outcome = /<(failure|error)\b/.test(body)
      ? "failed"
      : /<skipped\b/.test(body)
        ? "skipped"
        : "passed";
    cases.push({ classname: attrs.get("classname") ?? "", name: attrs.get("name") ?? "", outcome });
  }
  return cases;
}
