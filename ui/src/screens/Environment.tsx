import { useId, useState } from "react";

import type { Api } from "../api";
import type { AlarmEntry, TrendData } from "../bindings";
import { useLoad } from "../components/useLoad";
import { Card, EmptyState, NoData, SectionHead, Stat } from "../components/Values";
import { strings } from "../strings";
import { utcTime } from "../time";

const CABIN = ["cabin_co2", "cabin_o2", "cabin_pressure", "cabin_temp", "cabin_humidity"];
const RADIATION = ["radiation_dose_rate", "radiation_dose_cumulative"];
const EXERCISE = ["exercise_load", "exercise_duration"];
const BODY = ["respiration_rate", "heart_rate", "spo2", "body_temp_measured"];

const label = (p: string) => strings.parameter[p] ?? p;
const unitText = (u: string) => strings.unit[u] ?? u;

type Point = { readonly t: number; readonly v: number };

function points(trend: TrendData): Point[] {
  return trend.points.flatMap((p) => (p.value !== null && p.measuredAtUtcMs !== null ? [{ t: p.measuredAtUtcMs, v: p.value }] : []));
}

/**
 * Correlation (F-ENV-04): a cabin value and a body value on one time axis, with the environment
 * alarms of the period marked on both, so a change in one can be read against the other. Each
 * series keeps its own value scale; nothing is interpolated or smoothed.
 */
export function CorrelationChart({
  env,
  body,
  envParam,
  bodyParam,
  events,
}: {
  readonly env: TrendData;
  readonly body: TrendData;
  readonly envParam: string;
  readonly bodyParam: string;
  readonly events: readonly AlarmEntry[];
}) {
  const e = points(env);
  const b = points(body);
  const marks = events.flatMap((a) => (a.createdAtUtcMs === null ? [] : [{ t: a.createdAtUtcMs, title: a.title ?? a.category }]));
  const times = [...e, ...b].map((p) => p.t).concat(marks.map((m) => m.t));
  if (e.length === 0 && b.length === 0) return <NoData status="noData" />;
  const t0 = Math.min(...times);
  const t1 = Math.max(...times);
  const tspan = t1 - t0 || 1;
  const x = (t: number) => (((t - t0) / tspan) * 1000).toFixed(1);
  const lane = (ps: readonly Point[], top: number) => {
    if (ps.length === 0) return null;
    const lo = Math.min(...ps.map((p) => p.v));
    const span = Math.max(...ps.map((p) => p.v)) - lo || 1;
    const y = (v: number) => (top + 85 - ((v - lo) / span) * 80).toFixed(1);
    return <polyline points={ps.map((p) => `${x(p.t)},${y(p.v)}`).join(" ")} fill="none" stroke="currentColor" strokeWidth="2" vectorEffect="non-scaling-stroke" />;
  };
  const lastOf = (ps: readonly Point[], unit: string) => {
    const l = ps[ps.length - 1];
    return l === undefined ? strings.noData.noData : `${String(l.v)} ${unitText(unit)} ${utcTime(l.t)}`;
  };
  const envUnit = env.points[0]?.unit ?? "";
  const bodyUnit = body.points[0]?.unit ?? "";
  const summary = strings.envCorrelationLabel(label(envParam), e.length, label(bodyParam), b.length, marks.length, utcTime(t0), utcTime(t1));
  return (
    <div className="flex flex-col gap-2">
      <svg role="img" aria-label={summary} viewBox="0 0 1000 200" className="h-56 w-full" preserveAspectRatio="none">
        <line x1="0" x2="1000" y1="100" y2="100" className="stroke-ink-3" strokeWidth="1" vectorEffect="non-scaling-stroke" />
        {lane(e, 0)}
        {lane(b, 100)}
        {marks.map((m) => (
          <line key={`${String(m.t)}-${m.title}`} x1={x(m.t)} x2={x(m.t)} y1="0" y2="200" stroke="currentColor" strokeDasharray="6 4" strokeWidth="2" vectorEffect="non-scaling-stroke" />
        ))}
      </svg>
      <p className="text-sm">{strings.envCorrelationLane(label(envParam), lastOf(e, envUnit))}</p>
      <p className="text-sm">{strings.envCorrelationLane(label(bodyParam), lastOf(b, bodyUnit))}</p>
      {marks.length > 0 ? (
        <ul className="text-sm">
          {marks.map((m) => (
            <li key={`${String(m.t)}-${m.title}`}>{strings.envCorrelationEvent(m.title, utcTime(m.t))}</li>
          ))}
        </ul>
      ) : (
        <p className="text-sm text-ink-3">{strings.envCorrelationNoEvents}</p>
      )}
    </div>
  );
}

/** Environment (docs/07 §4; F-ENV-01..04): cabin, radiation, exercise, correlation with physiology. */
export function Environment({ api, crewId, version }: { readonly api: Api; readonly crewId: string | null; readonly version: number }) {
  const envId = useId();
  const bodyId = useId();
  const [envParam, setEnvParam] = useState("cabin_co2");
  const [bodyParam, setBodyParam] = useState("respiration_rate");
  const key = `${crewId ?? ""}:${String(version)}`;
  const latest = useLoad(crewId === null ? null : () => api.vitalsLatest(crewId), key);
  const envTrend = useLoad(crewId === null ? null : () => api.vitalsTrend(crewId, envParam, 21600), `${key}:${envParam}`);
  const bodyTrend = useLoad(crewId === null ? null : () => api.vitalsTrend(crewId, bodyParam, 21600), `${key}:${bodyParam}`);
  const alarms = useLoad(crewId === null ? null : () => api.alarmList(), key);
  if (crewId === null) return <EmptyState detail={strings.chooseCrew} />;
  if (latest.state === "error") {
    return (
      <p role="alert" className="text-sm font-semibold">
        {latest.message}
      </p>
    );
  }
  const values = latest.state === "ok" ? latest.data : [];
  const stat = (p: string) => {
    const v = values.find((x) => x.reading.parameter === p);
    if (v === undefined || v.reading.value === null) return <Stat key={p} label={label(p)} reading={null} />;
    if (v.status !== "live") return <Stat key={p} label={label(p)} reading={null} missing={v.status === "stale" ? "stale" : "unusable"} />;
    return (
      <Stat
        key={p}
        label={label(p)}
        reading={{ value: v.reading.value, unit: unitText(v.reading.unit), at: utcTime(v.reading.measuredAtUtcMs), source: v.deviceModel ?? strings.sourceUnknown }}
      />
    );
  };
  const anyVehicle = values.some((v) => [...CABIN, ...RADIATION, ...EXERCISE].includes(v.reading.parameter));
  const select = (id: string, value: string, set: (v: string) => void, options: readonly string[], title: string) => (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className="text-sm font-semibold">
        {title}
      </label>
      <select
        id={id}
        className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
        value={value}
        onChange={(e) => {
          set(e.target.value);
        }}
      >
        {options.map((o) => (
          <option key={o} value={o}>
            {label(o)}
          </option>
        ))}
      </select>
    </div>
  );
  return (
    <div className="flex flex-col gap-6">
      {anyVehicle ? null : <p className="text-sm text-ink-3">{strings.envNoVehicle}</p>}
      <Card label={strings.envCabin}>
        <SectionHead title={strings.envCabin} />
        <div className="mt-4 grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-6">{CABIN.map(stat)}</div>
      </Card>
      <Card label={strings.envRadiation}>
        <SectionHead title={strings.envRadiation} />
        <div className="mt-4 grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-6">
          {RADIATION.map(stat)}
          <Stat label={label("spe_state")} reading={null} />
        </div>
      </Card>
      <Card label={strings.envExercise}>
        <SectionHead title={strings.envExercise} />
        <div className="mt-4 grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-6">{EXERCISE.map(stat)}</div>
      </Card>
      <Card label={strings.envCorrelation}>
        <SectionHead title={strings.envCorrelation} />
        <p className="mt-2 text-sm text-ink-3">{strings.envCorrelationHelp}</p>
        <div className="mt-4 flex flex-wrap gap-4">
          {select(envId, envParam, setEnvParam, [...CABIN, ...RADIATION], strings.envEnvParam)}
          {select(bodyId, bodyParam, setBodyParam, BODY, strings.envBodyParam)}
        </div>
        <div className="mt-4">
          {envTrend.state === "ok" && bodyTrend.state === "ok" ? (
            <CorrelationChart
              env={envTrend.data}
              body={bodyTrend.data}
              envParam={envParam}
              bodyParam={bodyParam}
              events={alarms.state === "ok" ? alarms.data.filter((a) => a.category === "ENVIRONMENT" && a.crewMemberId === crewId) : []}
            />
          ) : (
            <NoData status="noData" />
          )}
        </div>
      </Card>
    </div>
  );
}
