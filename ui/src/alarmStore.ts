import type { AlarmBusEvent } from "./bindings";

/**
 * The station's view of open alarms, built only from alarm-bus events (no values, no why:
 * those need an authorised command). Pure reducer, so it is tested without the app.
 */
export interface AlarmView {
  readonly alarm: string;
  readonly crewId: string;
  readonly priority: string;
  readonly category: string;
  readonly state: string;
  readonly title: string;
  readonly stages: readonly { readonly stage: number; readonly audience: string }[];
  readonly sounding: boolean;
  /** Silenced (SR-008) rather than acknowledged or not yet sounding. */
  readonly silenced: boolean;
}

export type AlarmMap = ReadonlyMap<string, AlarmView>;

const RANK: Readonly<Record<string, number>> = { critical: 3, warning: 2, observe: 1 };

export function reduce(state: AlarmMap, e: AlarmBusEvent): AlarmMap {
  const next = new Map(state);
  const current = next.get(e.alarm);
  switch (e.type) {
    case "alarm":
      next.set(e.alarm, {
        alarm: e.alarm,
        crewId: e.crewId,
        priority: e.priority,
        category: e.category,
        state: e.state,
        title: e.title,
        stages: current?.stages ?? [],
        sounding: current?.sounding ?? false,
        silenced: current?.silenced ?? false,
      });
      return next;
    case "ended":
      next.delete(e.alarm);
      return next;
    case "stage":
      if (current === undefined) return state;
      next.set(e.alarm, { ...current, stages: [...current.stages, { stage: e.stage, audience: e.audience }] });
      return next;
    case "sound":
      if (current === undefined) return state;
      next.set(e.alarm, { ...current, sounding: true, silenced: false });
      return next;
    case "soundOff":
      if (current === undefined) return state;
      next.set(e.alarm, { ...current, sounding: false, silenced: e.reason === "silenced" });
      return next;
  }
}

/** Open alarms, highest priority first, then oldest first (insertion order). */
export function ranked(state: AlarmMap): AlarmView[] {
  return [...state.values()].sort((a, b) => (RANK[b.priority] ?? 0) - (RANK[a.priority] ?? 0));
}
