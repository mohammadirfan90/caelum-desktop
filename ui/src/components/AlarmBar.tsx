import { useEffect, useState } from "react";

import type { AlarmView } from "../alarmStore";
import type { Prefs } from "../settings";
import { strings } from "../strings";
import { type Level, LevelGlyph } from "./LevelGlyph";

/** Plan stages in order, for the tracker (docs/06 §3; delivered ones come from the bus). */
export function EscalationTracker({ alarm }: { readonly alarm: AlarmView }) {
  const delivered = new Set(alarm.stages.map((s) => s.audience));
  const audiences = Object.keys(strings.audience) as (keyof typeof strings.audience)[];
  const shown = audiences.filter((a) => delivered.has(a));
  return (
    <ol aria-label={strings.escalationLabel} className="flex flex-wrap gap-2 text-xs">
      {shown.map((a) => (
        <li key={a} className="rounded-full border border-border px-2.5 py-1">
          <span className="font-semibold">{strings.audience[a]}</span>
          <span className="sr-only">{` ${strings.stageDelivered}`}</span>
        </li>
      ))}
    </ol>
  );
}

function stateText(state: string): string {
  return state in strings.alarmState ? strings.alarmState[state as keyof typeof strings.alarmState] : state;
}

export function level(priority: string): Level {
  return priority === "critical" || priority === "warning" || priority === "observe" ? priority : "technical";
}

/**
 * Always-visible alarm bar (docs/07 §3): the highest active alarm, how many more, and
 * acknowledge/silence. Shapes and text carry the level; colour is only added. Actions are offered
 * to everyone; the core decides who may act (ROLE_MATRIX §2) and refusals are shown by the caller.
 */
export function AlarmBar({
  alarms,
  onAcknowledge,
  onSilence,
  prefs,
  onPrefs,
}: {
  readonly alarms: readonly AlarmView[];
  readonly onAcknowledge: (alarm: string) => void;
  readonly onSilence: (alarm: string) => void;
  readonly prefs?: Prefs;
  readonly onPrefs?: (p: Prefs) => void;
}) {
  const top = alarms[0];
  const [time, setTime] = useState(() => new Date().toISOString().substring(11, 19));

  useEffect(() => {
    const timer = setInterval(() => {
      setTime(new Date().toISOString().substring(11, 19));
    }, 1000);
    return () => clearInterval(timer);
  }, []);

  return (
    <section
      aria-label={strings.alarmBarLabel}
      className="flex min-h-14 flex-wrap items-center justify-between gap-4 border-b border-border bg-surface/90 px-4 sm:px-6 py-2 backdrop-blur-md"
    >
      <div className="flex flex-wrap items-center gap-4 min-w-0">
        {top === undefined ? (
          <p className="flex items-center gap-2 text-sm text-ink-3">
            <LevelGlyph level="normal" size={16} />
            <span className="font-medium tracking-wide">{strings.noActiveAlarms}</span>
          </p>
        ) : (
          <>
            <div aria-live="assertive" className="flex min-w-[16rem] flex-1 items-center gap-3">
              <LevelGlyph level={level(top.priority)} size={28} />
              <div className="min-w-0">
                <p className="text-base font-semibold break-words">
                  {`${strings.level[level(top.priority)]}: ${top.title}`}
                </p>
                <p className="text-sm text-ink-3">
                  {stateText(top.state)}
                  {top.silenced ? ` · ${strings.silenced}` : null}
                </p>
              </div>
              {alarms.length > 1 ? <span className="text-sm font-semibold">{strings.moreAlarms(alarms.length - 1)}</span> : null}
            </div>
            <EscalationTracker alarm={top} />
            <button
              type="button"
              className="btn btn-ink min-h-[38px] px-3.5 py-1 text-xs"
              onClick={() => {
                onAcknowledge(top.alarm);
              }}
            >
              {strings.acknowledge}
            </button>
            {top.priority === "observe" ? null : (
              <button
                type="button"
                className="btn btn-soft min-h-[38px] px-3.5 py-1 text-xs"
                onClick={() => {
                  onSilence(top.alarm);
                }}
              >
                {strings.silence}
              </button>
            )}
          </>
        )}
      </div>

      <div className="flex items-center gap-3 ml-auto">
        <div className="hidden sm:flex items-center gap-1.5 font-mono text-xs text-ink-3">
          <span aria-hidden="true" className="size-2 rounded-full bg-signal animate-pulse" />
          <span className="tabular-nums font-semibold text-ink">{time}</span>
        </div>

        {prefs && onPrefs ? (
          <div className="flex items-center rounded-lg border border-border bg-inset/60 p-0.5 text-xs font-medium">
            <button
              type="button"
              title={strings.themes.dark}
              className={`rounded-md px-2.5 py-1 transition-all ${
                prefs.theme === "dark" ? "bg-surface text-ink font-semibold shadow-xs" : "text-ink-3 hover:text-ink"
              }`}
              onClick={() => onPrefs({ ...prefs, theme: "dark" })}
            >
              {strings.themes.dark}
            </button>
            <button
              type="button"
              title={strings.themes.light}
              className={`rounded-md px-2.5 py-1 transition-all ${
                prefs.theme === "light" ? "bg-surface text-ink font-semibold shadow-xs" : "text-ink-3 hover:text-ink"
              }`}
              onClick={() => onPrefs({ ...prefs, theme: "light" })}
            >
              {strings.themes.light}
            </button>
            <button
              type="button"
              title={strings.themes["high-contrast"]}
              className={`rounded-md px-2.5 py-1 transition-all ${
                prefs.theme === "high-contrast" ? "bg-surface text-ink font-semibold shadow-xs" : "text-ink-3 hover:text-ink"
              }`}
              onClick={() => onPrefs({ ...prefs, theme: "high-contrast" })}
            >
              {strings.themes["high-contrast"]}
            </button>
          </div>
        ) : null}
      </div>
    </section>
  );
}
