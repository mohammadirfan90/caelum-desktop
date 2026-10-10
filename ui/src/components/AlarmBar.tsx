import type { AlarmView } from "../alarmStore";
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

function level(priority: string): Level {
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
}: {
  readonly alarms: readonly AlarmView[];
  readonly onAcknowledge: (alarm: string) => void;
  readonly onSilence: (alarm: string) => void;
}) {
  const top = alarms[0];
  return (
    <section aria-label={strings.alarmBarLabel} className="flex min-h-16 items-center gap-4 border-b border-border bg-surface px-4 py-2">
      {top === undefined ? (
        <p className="flex items-center gap-2 text-sm text-ink-3">
          <LevelGlyph level="normal" size={16} />
          {strings.noActiveAlarms}
        </p>
      ) : (
        <>
          <div aria-live="assertive" className="flex min-w-0 flex-1 items-center gap-3">
            <LevelGlyph level={level(top.priority)} size={28} />
            <div className="min-w-0">
              <p className="truncate text-base font-semibold">
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
          <button type="button" className="btn btn-ink" onClick={() => {
              onAcknowledge(top.alarm);
            }}>
            {strings.acknowledge}
          </button>
          {top.priority === "observe" ? null : (
            <button type="button" className="btn btn-soft" onClick={() => {
                onSilence(top.alarm);
              }}>
              {strings.silence}
            </button>
          )}
        </>
      )}
    </section>
  );
}
