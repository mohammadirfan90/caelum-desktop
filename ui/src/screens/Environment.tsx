import { useId, useState } from "react";

import type { Api } from "../api";
import { useLoad } from "../components/useLoad";
import { Card, EmptyState, NoData, SectionHead, Stat } from "../components/Values";
import { strings } from "../strings";
import { utcTime } from "../time";
import { TrendChart } from "./Crew";

const CABIN = ["cabin_co2", "cabin_o2", "cabin_pressure", "cabin_temp", "cabin_humidity"];
const RADIATION = ["radiation_dose_rate", "radiation_dose_cumulative"];
const EXERCISE = ["exercise_load", "exercise_duration"];
const BODY = ["respiration_rate", "heart_rate", "spo2", "body_temp_measured"];

const label = (p: string) => strings.parameter[p] ?? p;
const unitText = (u: string) => strings.unit[u] ?? u;

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
        <div className="mt-4 grid gap-6 md:grid-cols-2">
          {envTrend.state === "ok" ? <TrendChart trend={envTrend.data} parameter={envParam} /> : <NoData status="noData" />}
          {bodyTrend.state === "ok" ? <TrendChart trend={bodyTrend.data} parameter={bodyParam} /> : <NoData status="noData" />}
        </div>
      </Card>
    </div>
  );
}
