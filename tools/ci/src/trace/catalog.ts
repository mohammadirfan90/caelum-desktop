// trace: ignore-ids (this file documents the ID formats with examples; it cites none)
// The requirement catalog, read from the docs that define the IDs:
//   01_FEATURE_LOCK.md      F-*  with priority (M, S, v1.1, v2)
//   02_NASA_REQUIREMENTS.md R-*  and N-* (with the features that satisfy each need)
//   06_SAFETY_AND_ALARMS.md SR-*
//   10_BUILD_PLAN.md        milestone of each ID (from the milestone headings) and milestone status
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { extractIds } from "./ids.ts";

export type Kind = "feature" | "requirement" | "safety" | "need";

export interface CatalogEntry {
  readonly id: string;
  readonly kind: Kind;
  /** Feature priority from the feature lock (features only). */
  readonly priority?: string;
  /** Features that satisfy a need (needs only). */
  readonly features: readonly string[];
  readonly milestones: readonly string[];
}

export interface Catalog {
  readonly entries: ReadonlyMap<string, CatalogEntry>;
  /** Milestone → status text from the build plan's status table, e.g. "complete". */
  readonly milestoneStatus: ReadonlyMap<string, string>;
}

function tableRows(markdown: string, idPattern: RegExp): string[][] {
  return markdown
    .split(/\r?\n/)
    .filter((line) => line.startsWith("|"))
    .map((line) => line.split("|").slice(1, -1).map((cell) => cell.trim()))
    .filter((cells) => idPattern.test(cells[0] ?? ""));
}

/** Milestone → IDs named in its heading, e.g. "## M2. ... (F-ACQ-01/02, R-CLN-02)". */
export function milestoneIds(buildPlan: string): Map<string, string[]> {
  const result = new Map<string, string[]>();
  for (const m of buildPlan.matchAll(/^## (M\d+)\. .*$/gm)) {
    const milestone = m[1] ?? "";
    result.set(milestone, extractIds(m[0]));
  }
  return result;
}

export function milestoneStatus(buildPlan: string): Map<string, string> {
  const result = new Map<string, string>();
  for (const cells of tableRows(buildPlan, /^M\d+$/)) {
    result.set(cells[0] ?? "", (cells[1] ?? "").toLowerCase());
  }
  return result;
}

export function parseCatalog(docs: {
  featureLock: string;
  requirements: string;
  safety: string;
  buildPlan: string;
}): Catalog {
  const byMilestone = milestoneIds(docs.buildPlan);
  const milestonesOf = (id: string) =>
    [...byMilestone].filter(([, ids]) => ids.includes(id)).map(([m]) => m);

  const entries = new Map<string, CatalogEntry>();
  const add = (entry: Omit<CatalogEntry, "milestones">) => {
    entries.set(entry.id, { ...entry, milestones: milestonesOf(entry.id) });
  };

  for (const cells of tableRows(docs.featureLock, /^F-[A-Z]{2,4}-\d{2}$/)) {
    add({ id: cells[0] ?? "", kind: "feature", priority: cells[2] ?? "", features: [] });
  }
  for (const cells of tableRows(docs.requirements, /^R-[A-Z]{3}-\d{2}$/)) {
    add({ id: cells[0] ?? "", kind: "requirement", features: [] });
  }
  for (const cells of tableRows(docs.requirements, /^N-\d{2}$/)) {
    const features = extractIds(cells[2] ?? "").filter((id) => id.startsWith("F-"));
    add({ id: cells[0] ?? "", kind: "need", features });
  }
  for (const cells of tableRows(docs.safety, /^SR-\d{3}$/)) {
    add({ id: cells[0] ?? "", kind: "safety", features: [] });
  }
  return { entries, milestoneStatus: milestoneStatus(docs.buildPlan) };
}

export function readCatalog(repoRoot: string): Catalog {
  const doc = (name: string) => readFileSync(join(repoRoot, "docs", name), "utf8");
  return parseCatalog({
    featureLock: doc("01_FEATURE_LOCK.md"),
    requirements: doc("02_NASA_REQUIREMENTS.md"),
    safety: doc("06_SAFETY_AND_ALARMS.md"),
    buildPlan: doc("10_BUILD_PLAN.md"),
  });
}
