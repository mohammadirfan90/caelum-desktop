import { useId, useState } from "react";

import type { AlarmView } from "../alarmStore";
import type { Api } from "../api";
import type { TrendData, VitalItem, WaveformChunk } from "../bindings";
import { level } from "../components/AlarmBar";
import { LevelBadge, LevelGlyph } from "../components/LevelGlyph";
import { useLoad } from "../components/useLoad";
import { Card, EmptyState, NoData, type Reading, SectionHead, Stat } from "../components/Values";
import { strings } from "../strings";
import { utcTime } from "../time";

/** What the crew screens show about one crew member. */
export interface CrewContext {
  readonly api: Api;
  /** Selected crew member, or null when none is chosen. */
  readonly crewId: string | null;
  /** Changes whenever new readings for the selected crew member arrive (reload key). */
  readonly version: number;
  /** Open alarms from the alarm bus (no values). */
  readonly alarms: readonly AlarmView[];
}

const label = (p: string) => strings.parameter[p] ?? p;
const unitText = (u: string) => strings.unit[u] ?? u;

/** A stored value as a Stat: a number only with its source and time (M5 gate), else why not. */
function VitalStat({ parameter, vitals }: { readonly parameter: string; readonly vitals: readonly VitalItem[] }) {
  const v = vitals.find((x) => x.reading.parameter === parameter);
  if (v === undefined) return <Stat label={label(parameter)} reading={null} missing="noData" />;
  if (v.status === "unusable") return <Stat label={label(parameter)} reading={null} missing="unusable" />;
  const value = v.reading.value;
  if (value === null) return <Stat label={label(parameter)} reading={null} missing="noData" />;
  const at = utcTime(v.reading.measuredAtUtcMs);
  const reading: Reading = {
    value,
    unit: unitText(v.reading.unit),
    at: v.status === "stale" ? strings.staleSince(strings.ago(v.ageS)) : at,
    source: v.deviceModel ?? strings.sourceUnknown,
  };
  if (v.status === "stale") {
    // SR-004: a stale value is never shown as current; the age says how old the last one is.
    return (
      <div className="flex flex-col gap-1">
        <Stat label={label(parameter)} reading={null} missing="stale" />
        <span className="text-xs text-ink-3">{strings.sourceAt(reading.source, `${at} (${strings.ago(v.ageS)})`)}</span>
      </div>
    );
  }
  return <Stat label={label(parameter)} reading={reading} />;
}

function Vitals({ params, vitals }: { readonly params: readonly string[]; readonly vitals: readonly VitalItem[] }) {
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-6">
      {params.map((p) => (
        <VitalStat key={p} parameter={p} vitals={vitals} />
      ))}
    </div>
  );
}

function NeedCrew() {
  return <EmptyState detail={strings.chooseCrew} />;
}

function Problem({ message }: { readonly message: string }) {
  return (
    <p role="alert" className="text-sm font-semibold">
      {message}
    </p>
  );
}

/** Home (docs/07 §4): status level, live vitals, active alarms, next check, environment. */
export function Home({ ctx }: { readonly ctx: CrewContext }) {
  const { api, crewId } = ctx;
  const vitals = useLoad(crewId === null ? null : () => api.vitalsLatest(crewId), `${crewId ?? ""}:${String(ctx.version)}`);
  if (crewId === null) return <NeedCrew />;
  const mine = ctx.alarms.filter((a) => a.crewId === crewId);
  const top = mine[0];
  return (
    <div className="flex flex-col gap-6">
      <Card label={strings.homeStatus}>
        <SectionHead title={strings.homeStatus} />
        <div className="mt-3">
          <LevelBadge level={top === undefined ? "normal" : level(top.priority)} />
        </div>
      </Card>
      <Card label={strings.homeVitals}>
        <SectionHead title={strings.homeVitals} />
        <div className="mt-4">
          {vitals.state === "error" ? (
            <Problem message={vitals.message} />
          ) : (
            <Vitals
              params={["heart_rate", "spo2", "pulse_rate", "respiration_rate", "body_temp_measured", "bp_systolic", "bp_diastolic"]}
              vitals={vitals.state === "ok" ? vitals.data : []}
            />
          )}
        </div>
      </Card>
      <Card label={strings.homeAlarms}>
        <SectionHead title={strings.homeAlarms} />
        {mine.length === 0 ? (
          <p className="mt-3 text-sm text-ink-3">{strings.homeNoAlarms}</p>
        ) : (
          <ul className="mt-3 flex flex-col gap-2">
            {mine.map((a) => (
              <li key={a.alarm} className="flex items-center gap-3">
                <LevelGlyph level={level(a.priority)} />
                <span className="font-semibold">{`${strings.level[level(a.priority)]}: ${a.title}`}</span>
              </li>
            ))}
          </ul>
        )}
      </Card>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-6">
        <Card label={strings.homeNextCheck}>
          <SectionHead title={strings.homeNextCheck} />
          <div className="mt-3">
            <NoData status="noData" />
          </div>
        </Card>
        <Card label={strings.homeEnvironment}>
          <SectionHead title={strings.homeEnvironment} />
          <div className="mt-3">
            <NoData status="noData" />
          </div>
        </Card>
      </div>
    </div>
  );
}

/** ECG strip from stored waveform chunks (`i16le` samples); NO DATA when there are none. */
function EcgStrip({ chunks }: { readonly chunks: readonly WaveformChunk[] }) {
  const samples: number[] = [];
  for (const c of chunks) {
    if (c.encoding !== "i16le" || c.quality === "unusable") continue;
    for (let i = 0; i + 1 < c.data.length; i += 2) {
      const lo = c.data[i] ?? 0;
      const hi = c.data[i + 1] ?? 0;
      const v = (hi << 8) | lo;
      samples.push(v >= 0x8000 ? v - 0x10000 : v);
    }
  }
  if (samples.length < 2) return <NoData status="noData" />;
  const min = Math.min(...samples);
  const span = Math.max(...samples) - min || 1;
  const points = samples.map((v, i) => `${((i / (samples.length - 1)) * 1000).toFixed(1)},${(100 - ((v - min) / span) * 100).toFixed(1)}`).join(" ");
  return (
    <svg role="img" aria-label={strings.ecgLabel(samples.length)} viewBox="0 0 1000 100" className="h-32 w-full" preserveAspectRatio="none">
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth="2" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

/** Trend chart: the readings over the window and the personal range when a baseline exists. */
function TrendChart({ trend, parameter }: { readonly trend: TrendData; readonly parameter: string }) {
  const pts = trend.points.filter((p) => p.value !== null && p.measuredAtUtcMs !== null);
  const b = trend.baseline;
  if (pts.length === 0) return <p className="text-sm text-ink-3">{strings.trendNone}</p>;
  const values = pts.map((p) => p.value ?? 0);
  const band = b !== null && b.median !== null && b.mad !== null ? { lo: b.median - 2.5 * 1.4826 * b.mad, hi: b.median + 2.5 * 1.4826 * b.mad } : null;
  const lo = Math.min(...values, band?.lo ?? Infinity);
  const hi = Math.max(...values, band?.hi ?? -Infinity);
  const span = hi - lo || 1;
  const t0 = pts[0]?.measuredAtUtcMs ?? 0;
  const t1 = pts[pts.length - 1]?.measuredAtUtcMs ?? t0;
  const tspan = t1 - t0 || 1;
  const y = (v: number) => 100 - ((v - lo) / span) * 90 - 5;
  const x = (t: number) => ((t - t0) / tspan) * 1000;
  const last = pts[pts.length - 1];
  const summary = strings.trendLabel(label(parameter), pts.length, last === undefined ? "" : `${String(last.value)} ${unitText(last.unit)} ${utcTime(last.measuredAtUtcMs)}`);
  return (
    <div className="flex flex-col gap-2">
      <svg role="img" aria-label={summary} viewBox="0 0 1000 100" className="h-40 w-full" preserveAspectRatio="none">
        {band === null ? null : <rect x="0" width="1000" y={y(band.hi)} height={Math.max(1, y(band.lo) - y(band.hi))} className="fill-inset" />}
        <polyline
          points={pts.map((p) => `${x(p.measuredAtUtcMs ?? t0).toFixed(1)},${y(p.value ?? 0).toFixed(1)}`).join(" ")}
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
      <p className="text-sm text-ink-3">{band === null ? strings.baselineNone : strings.baselineBand}</p>
    </div>
  );
}

const TREND_PARAMS = ["heart_rate", "spo2", "respiration_rate", "body_temp_measured", "hrv_rmssd", "bp_systolic", "weight"];

/** Body (docs/07 §4): heart with live ECG, breathing, temperature, sleep, trends vs baseline. */
export function Body({ ctx }: { readonly ctx: CrewContext }) {
  const { api, crewId } = ctx;
  const paramId = useId();
  const windowId = useId();
  const [parameter, setParameter] = useState("heart_rate");
  const [windowS, setWindowS] = useState(3600);
  const key = `${crewId ?? ""}:${String(ctx.version)}`;
  const vitals = useLoad(crewId === null ? null : () => api.vitalsLatest(crewId), key);
  const ecg = useLoad(crewId === null ? null : () => api.waveformRecent(crewId, "ecg", 10), key);
  const trend = useLoad(crewId === null ? null : () => api.vitalsTrend(crewId, parameter, windowS), `${key}:${parameter}:${String(windowS)}`);
  if (crewId === null) return <NeedCrew />;
  const v = vitals.state === "ok" ? vitals.data : [];
  return (
    <div className="flex flex-col gap-6">
      {vitals.state === "error" ? <Problem message={vitals.message} /> : null}
      <Card label={strings.bodyHeart}>
        <SectionHead title={strings.bodyHeart} />
        <div className="mt-4 flex flex-col gap-4">
          <Vitals params={["heart_rate", "hrv_rmssd", "hrv_sdnn"]} vitals={v} />
          <h3 className="text-base font-semibold">{strings.ecgTitle}</h3>
          {ecg.state === "ok" ? <EcgStrip chunks={ecg.data} /> : <NoData status="noData" />}
        </div>
      </Card>
      <Card label={strings.bodyBreathing}>
        <SectionHead title={strings.bodyBreathing} />
        <div className="mt-4">
          <Vitals params={["respiration_rate", "spo2", "pulse_rate"]} vitals={v} />
        </div>
      </Card>
      <Card label={strings.bodyTemperature}>
        <SectionHead title={strings.bodyTemperature} />
        <div className="mt-4">
          <Vitals params={["body_temp_measured", "core_temp", "skin_temp_proximal", "skin_temp_distal"]} vitals={v} />
        </div>
      </Card>
      <Card label={strings.bodyPressure}>
        <SectionHead title={strings.bodyPressure} />
        <div className="mt-4">
          <Vitals params={["bp_systolic", "bp_diastolic"]} vitals={v} />
        </div>
      </Card>
      <Card label={strings.bodySleep}>
        <SectionHead title={strings.bodySleep} />
        <div className="mt-4">
          <Vitals params={["sleep_stage", "activity_level"]} vitals={v} />
        </div>
      </Card>
      <Card label={strings.trendTitle}>
        <SectionHead title={strings.trendTitle} />
        <div className="mt-4 flex flex-wrap gap-4">
          <div className="flex flex-col gap-1">
            <label htmlFor={paramId} className="text-sm font-semibold">
              {strings.trendParameter}
            </label>
            <select
              id={paramId}
              className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
              value={parameter}
              onChange={(e) => {
                setParameter(e.target.value);
              }}
            >
              {TREND_PARAMS.map((p) => (
                <option key={p} value={p}>
                  {label(p)}
                </option>
              ))}
            </select>
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor={windowId} className="text-sm font-semibold">
              {strings.trendWindow}
            </label>
            <select
              id={windowId}
              className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
              value={windowS}
              onChange={(e) => {
                setWindowS(Number(e.target.value));
              }}
            >
              {[3600, 21600, 86400].map((w) => (
                <option key={w} value={w}>
                  {strings.trendWindows[w]}
                </option>
              ))}
            </select>
          </div>
        </div>
        <div className="mt-4">
          {trend.state === "ok" ? <TrendChart trend={trend.data} parameter={parameter} /> : trend.state === "error" ? <Problem message={trend.message} /> : null}
        </div>
      </Card>
    </div>
  );
}
