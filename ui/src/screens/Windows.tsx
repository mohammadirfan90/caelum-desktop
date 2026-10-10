import { useEffect, useId, useState } from "react";

import type { AlarmView } from "../alarmStore";
import type { Api, DataEvents } from "../api";
import type { CrewMember, SessionInfo } from "../bindings";
import { level } from "../components/AlarmBar";
import { LevelGlyph } from "../components/LevelGlyph";
import { useLoad } from "../components/useLoad";
import { EmptyState, NoData } from "../components/Values";
import { strings } from "../strings";
import { EcgStrip } from "./Crew";

/**
 * Kiosk crew display (F-SYS-13, F-ALM-07): the crew-wide alarm board for a wall-mounted
 * screen, from the alarm bus only (no values, no sign-in). Large type, shapes and text.
 */
export function KioskBoard({ alarms }: { readonly alarms: readonly AlarmView[] }) {
  return (
    <section aria-labelledby="kiosk-title" className="flex w-full flex-col gap-6 p-8">
      <h2 id="kiosk-title" className="text-4xl font-semibold">
        {strings.kioskTitle}
      </h2>
      {alarms.length === 0 ? (
        <p className="flex items-center gap-3 text-3xl">
          <LevelGlyph level="normal" size={36} />
          {strings.emergencyNoAlarms}
        </p>
      ) : (
        <ul className="flex flex-col gap-4">
          {alarms.map((a) => (
            <li key={a.alarm} className="card flex items-center gap-4 p-6 text-3xl font-semibold">
              <LevelGlyph level={level(a.priority)} size={44} />
              {`${strings.level[level(a.priority)]}: ${a.title}`}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** The waveform viewer window (F-SYS-13, F-CLN-03): a live ECG strip on its own screen. */
export function WaveformWindow({ api, events }: { readonly api: Api; readonly events: DataEvents }) {
  const selectId = useId();
  const [session, setSession] = useState<SessionInfo | null>(null);
  const [crew, setCrew] = useState<CrewMember[]>([]);
  const [chosen, setChosen] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    void api.authMe().then((r) => {
      if (r.status === "ok") setSession(r.data);
    });
    void api.crewList().then((r) => {
      if (r.status === "ok") setCrew(r.data);
    });
  }, [api]);
  const crewId = session?.role === "astronaut" ? session.crewMemberId : chosen;
  useEffect(() => {
    let stop: (() => void) | undefined;
    void events
      .onObservations((id) => {
        if (id === crewId) setTick((t) => t + 1);
      })
      .then((s) => {
        stop = s;
      });
    const t = setInterval(() => {
      setTick((n) => n + 1);
    }, 1000);
    return () => {
      clearInterval(t);
      stop?.();
    };
  }, [events, crewId]);
  const ecg = useLoad(crewId === null ? null : () => api.waveformRecent(crewId, "ecg", 10), `${crewId ?? ""}:${String(tick)}`);
  return (
    <main id="content" className="flex min-h-full flex-col gap-4 bg-canvas p-6 text-ink">
      <h1 className="text-2xl font-semibold">{strings.waveformWindowTitle}</h1>
      {session === null ? <EmptyState detail={strings.errors.signInRequired} /> : null}
      {session !== null && session.role !== "astronaut" ? (
        <div className="flex items-center gap-2">
          <label htmlFor={selectId} className="text-sm font-semibold">
            {strings.crewSelect}
          </label>
          <select
            id={selectId}
            className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
            value={chosen ?? ""}
            onChange={(e) => {
              setChosen(e.target.value === "" ? null : e.target.value);
            }}
          >
            <option value="">{strings.crewSelectNone}</option>
            {crew
              .filter((c) => c.active)
              .map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
          </select>
        </div>
      ) : null}
      {crewId !== null && ecg.state === "ok" ? <EcgStrip chunks={ecg.data} /> : <NoData status="noData" />}
    </main>
  );
}
