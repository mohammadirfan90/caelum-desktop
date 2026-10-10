import type { ReactNode } from "react";

import { strings } from "../strings";

/** Why there is no value to show (CLAUDE.md rule 2, SR-004, SR-005). */
export type Missing = "noData" | "stale" | "unusable";

/** Shown instead of a number whenever there is no current, usable value. Never a made-up value. */
export function NoData({ status }: { readonly status: Missing }) {
  const text = strings.noData[status];
  return (
    <span className="inline-flex items-baseline gap-2" aria-label={strings.noDataLabel(text)}>
      <span aria-hidden="true" className="text-2xl font-light text-ink-3">
        —
      </span>
      <span className="text-xs font-semibold uppercase tracking-wider text-warning-ink">{text}</span>
    </span>
  );
}

/**
 * A value with its source. A number is shown only together with where it came from (device or
 * record) and when; without both, NoData is shown instead (M5 gate: no number without a source).
 */
export interface Reading {
  readonly value: number;
  readonly unit: string;
  /** Measurement time, already formatted for display (UTC/MET). */
  readonly at: string;
  /** Where it came from (device name, record). */
  readonly source: string;
}

export function Stat({
  label,
  reading,
  missing = "noData",
}: {
  readonly label: string;
  readonly reading: Reading | null;
  readonly missing?: Missing;
}) {
  const shown = reading !== null && reading.source.length > 0 && Number.isFinite(reading.value);
  return (
    <div className="flex flex-col gap-1">
      <span className="label">{label}</span>
      {shown ? (
        <span className="flex items-baseline gap-1.5">
          <span className="text-3xl font-semibold tabular-nums">{reading.value}</span>
          <span className="text-sm text-ink-3">{reading.unit}</span>
        </span>
      ) : (
        <NoData status={missing} />
      )}
      {shown ? <span className="text-xs text-ink-3">{`${reading.source} · ${reading.at}`}</span> : null}
    </div>
  );
}

/** Card surface. */
export function Card({ children, label }: { readonly children: ReactNode; readonly label?: string }) {
  return (
    <section className="card p-5" aria-label={label}>
      {children}
    </section>
  );
}

/** Section heading. */
export function SectionHead({ title, id }: { readonly title: string; readonly id?: string }) {
  return (
    <h2 id={id} className="text-lg font-semibold">
      {title}
    </h2>
  );
}

/** Nothing to show (no fabricated placeholder content). */
export function EmptyState({ title = strings.emptyTitle, detail }: { readonly title?: string; readonly detail?: string }) {
  return (
    <div className="flex flex-col items-start gap-1 py-6">
      <p className="text-base font-semibold">{title}</p>
      {detail === undefined ? null : <p className="text-sm text-ink-3">{detail}</p>}
    </div>
  );
}
