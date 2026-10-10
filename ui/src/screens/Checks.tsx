import { type SyntheticEvent, useEffect, useId, useRef, useState } from "react";

import type { Api } from "../api";
import { type Notice, NoticeLine } from "../components/Confirm";
import { useLoad } from "../components/useLoad";
import { Card, NoData, SectionHead } from "../components/Values";
import { strings } from "../strings";
import { utcDateTime } from "../time";
import { errorText } from "./Access";
import { Protocols } from "./Protocols";

/** PVT timing (CR-019 §3): 3-minute run, targets 2–10 s apart. */
export interface PvtTiming {
  readonly durationS: number;
  readonly minGapMs: number;
  readonly maxGapMs: number;
  /** Uniform [0, 1); the test protocol's randomness, never data. */
  readonly random: () => number;
  readonly now: () => number;
}

export const PVT_TIMING: PvtTiming = {
  durationS: 180,
  minGapMs: 2000,
  maxGapMs: 10000,
  random: Math.random,
  now: () => performance.now(),
};

type PvtPhase = { kind: "idle" } | { kind: "waiting" } | { kind: "target"; shownAt: number } | { kind: "done" };

/** Psychomotor vigilance test (F-CLN-06): reaction times measured here, the median stored by the core. */
export function PvtTest({ api, timing = PVT_TIMING }: { readonly api: Api; readonly timing?: PvtTiming }) {
  const [phase, setPhase] = useState<PvtPhase>({ kind: "idle" });
  const [times, setTimes] = useState<number[]>([]);
  const [left, setLeft] = useState(timing.durationS);
  const [notice, setNotice] = useState<Notice>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const endAt = useRef(0);
  const clear = () => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  };
  const finish = (all: number[]) => {
    clear();
    setPhase({ kind: "done" });
    void api.pvtSubmit(all).then((r) => {
      setNotice(r.status === "ok" ? { kind: "ok", text: strings.pvtResult(r.data.medianMs, r.data.lapses, r.data.responses) } : { kind: "error", text: errorText(r.error) });
    });
  };
  const schedule = (all: number[]) => {
    const gap = timing.minGapMs + timing.random() * (timing.maxGapMs - timing.minGapMs);
    if (timing.now() + gap >= endAt.current) {
      timer.current = setTimeout(() => {
        finish(all);
      }, Math.max(0, endAt.current - timing.now()));
      return;
    }
    setPhase({ kind: "waiting" });
    timer.current = setTimeout(() => {
      setPhase({ kind: "target", shownAt: timing.now() });
    }, gap);
  };
  useEffect(() => clear, []);
  useEffect(() => {
    if (phase.kind !== "waiting" && phase.kind !== "target") return;
    const t = setInterval(() => {
      setLeft(Math.max(0, Math.round((endAt.current - timing.now()) / 1000)));
    }, 1000);
    return () => {
      clearInterval(t);
    };
  }, [phase.kind, timing]);
  const start = () => {
    setTimes([]);
    setNotice(null);
    endAt.current = timing.now() + timing.durationS * 1000;
    setLeft(timing.durationS);
    schedule([]);
  };
  const respond = () => {
    if (phase.kind === "waiting") {
      setNotice({ kind: "error", text: strings.pvtTooSoon });
      return;
    }
    if (phase.kind !== "target") return;
    const rt = Math.round(timing.now() - phase.shownAt);
    const all = [...times, rt];
    setTimes(all);
    setNotice(null);
    schedule(all);
  };
  const running = phase.kind === "waiting" || phase.kind === "target";
  return (
    <Card label={strings.pvtTitle}>
      <SectionHead title={strings.pvtTitle} />
      <p className="mt-2 text-sm text-ink-3">{strings.pvtHelp}</p>
      {running ? (
        <div className="mt-4 flex flex-col items-start gap-3">
          <p aria-live="polite" className="text-sm">
            {strings.pvtProgress(times.length, left)}
          </p>
          <button
            type="button"
            className={`btn btn-lg ${phase.kind === "target" ? "btn-ink" : "btn-soft"}`}
            onClick={respond}
            onKeyDown={(e) => {
              if (e.key === " ") {
                e.preventDefault();
                respond();
              }
            }}
          >
            {phase.kind === "target" ? strings.pvtNow : strings.pvtWait}
          </button>
          <button
            type="button"
            className="btn btn-soft"
            onClick={() => {
              finish(times);
            }}
          >
            {strings.pvtStop}
          </button>
        </div>
      ) : (
        <button type="button" className="btn btn-ink mt-4" onClick={start}>
          {strings.pvtStart}
        </button>
      )}
      <NoticeLine notice={notice} />
    </Card>
  );
}

/** Daily private check-in (F-CLN-07): the crew member chooses who may read it. */
export function CheckinForm({ api }: { readonly api: Api }) {
  const ids = { mood: useId(), sleep: useId(), symptoms: useId(), tier: useId() };
  const [mood, setMood] = useState("3");
  const [sleep, setSleep] = useState("");
  const [symptoms, setSymptoms] = useState("");
  const [tier, setTier] = useState("private");
  const [notice, setNotice] = useState<Notice>(null);
  const submit = (e: SyntheticEvent) => {
    e.preventDefault();
    const answers = {
      mood: Number(mood),
      sleepHours: sleep.trim() === "" ? null : Number(sleep),
      symptoms: symptoms
        .split("\n")
        .map((s) => s.trim())
        .filter((s) => s !== ""),
    };
    void api.checkinSubmit(answers, tier).then((r) => {
      setNotice(r.status === "ok" ? { kind: "ok", text: strings.checkinDone } : { kind: "error", text: errorText(r.error) });
    });
  };
  return (
    <Card label={strings.checkinTitle}>
      <SectionHead title={strings.checkinTitle} />
      <form onSubmit={submit} aria-label={strings.checkinTitle} className="mt-3 flex max-w-[34rem] flex-col gap-3">
        <label htmlFor={ids.mood} className="text-sm font-semibold">
          {strings.checkinMood}
        </label>
        <select
          id={ids.mood}
          className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
          value={mood}
          onChange={(e) => {
            setMood(e.target.value);
          }}
        >
          {["1", "2", "3", "4", "5"].map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
        <label htmlFor={ids.sleep} className="text-sm font-semibold">
          {strings.checkinSleep}
        </label>
        <input
          id={ids.sleep}
          inputMode="decimal"
          className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
          value={sleep}
          onChange={(e) => {
            setSleep(e.target.value);
          }}
        />
        <label htmlFor={ids.symptoms} className="text-sm font-semibold">
          {strings.checkinSymptoms}
        </label>
        <textarea
          id={ids.symptoms}
          rows={3}
          className="rounded-lg border border-ink-3 bg-surface p-3"
          value={symptoms}
          onChange={(e) => {
            setSymptoms(e.target.value);
          }}
        />
        <label htmlFor={ids.tier} className="text-sm font-semibold">
          {strings.checkinTier}
        </label>
        <select
          id={ids.tier}
          className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
          value={tier}
          onChange={(e) => {
            setTier(e.target.value);
          }}
        >
          {Object.entries(strings.checkinTiers).map(([k, v]) => (
            <option key={k} value={k}>
              {v}
            </option>
          ))}
        </select>
        <NoticeLine notice={notice} />
        <button type="submit" className="btn btn-ink self-start">
          {strings.checkinSubmit}
        </button>
      </form>
    </Card>
  );
}

/** The next scheduled check (F-CLN-06, F-CLN-07): NO DATA without a signed schedule. */
function NextCheckCard({ api }: { readonly api: Api }) {
  const next = useLoad(() => api.scheduleNext(), "next");
  return (
    <Card label={strings.checksNext}>
      <SectionHead title={strings.checksNext} />
      <div className="mt-3">
        {next.state === "ok" && next.data !== null && next.data.atUtcMs !== null ? (
          <p className="text-lg">{strings.checksNextAt(strings.checkKinds[next.data.kind] ?? next.data.kind, utcDateTime(next.data.atUtcMs))}</p>
        ) : (
          <NoData status="noData" />
        )}
      </div>
    </Card>
  );
}

/** Checks (docs/07 §4): next scheduled check, PVT, daily check-in, own checklists. */
export function Checks({ api, crewId, isAstronaut }: { readonly api: Api; readonly crewId: string | null; readonly isAstronaut: boolean }) {
  return (
    <div className="flex flex-col gap-6">
      <NextCheckCard api={api} />
      {isAstronaut ? <PvtTest api={api} /> : null}
      {isAstronaut ? <CheckinForm api={api} /> : null}
      <Protocols api={api} crewId={crewId} canRun />
    </div>
  );
}
