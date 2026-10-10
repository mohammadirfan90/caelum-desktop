// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it } from 'vitest';
import {
  AlertTimelineModel,
  AlertTimelineView,
  alarmKey,
  createAlertTimelineViewProvider,
} from '../src/alert-timeline.js';
import { STATUS_PRESENTATION, type CrewEntry } from '../src/labels.js';
import { validateOpsAlarm, type OpsAlarm } from '../src/messages.js';
import { NAMESPACE, TYPE_ALERT_TIMELINE, TYPE_CREW_BOARD } from '../src/telemetry-provider.js';
import { useDom } from './dom.js';
import {
  CREW_A,
  CREW_B,
  FakeTime,
  FakeWebSocket,
  deferredFetch,
  fakeFetch,
  flush,
  opsAlarm,
  startedClient,
} from './fixtures.js';

const CREW: CrewEntry[] = [
  { id: CREW_A, label: 'Commander' },
  { id: CREW_B, label: 'Flight engineer' },
];
const BOUNDS = { start: 1_000, end: 2_000 };

function alarm(overrides: Record<string, unknown> = {}): OpsAlarm {
  const r = validateOpsAlarm(opsAlarm(overrides));
  if (!r.ok) throw new Error(r.reason);
  return r.message;
}

describe('F-GND-04 alert timeline model', () => {
  it('F-GND-04 groups by (crewId, createdAt, category) and orders state transitions', () => {
    const model = new AlertTimelineModel();
    // Added out of order: ended first, confirming last.
    model.add(alarm({ state: 'ended', at: '2026-10-10T12:00:09Z', seq: 4, escalationStage: 2 }));
    model.add(alarm({ state: 'active', at: '2026-10-10T12:00:03Z', seq: 2, escalationStage: 1 }));
    model.add(alarm({ state: 'acknowledged', at: '2026-10-10T12:00:05Z', seq: 3, escalationStage: 2 }));
    model.add(alarm({ state: 'confirming', at: '2026-10-10T12:00:01Z', seq: 1, escalationStage: 0 }));
    const groups = model.groups();
    expect(groups).toHaveLength(1);
    expect(groups[0]?.transitions.map((t) => t.state)).toEqual([
      'confirming',
      'active',
      'acknowledged',
      'ended',
    ]);
    expect(groups[0]?.transitions.map((t) => t.escalationStage)).toEqual([0, 1, 2, 2]);
  });

  it('F-GND-04 keeps different crew, category or creation time in separate groups', () => {
    const model = new AlertTimelineModel();
    model.add(alarm({ seq: 1 }));
    model.add(alarm({ seq: 2, crewId: CREW_B }));
    model.add(alarm({ seq: 3, category: 'THERMAL' }));
    model.add(alarm({ seq: 4, createdAt: '2026-10-10T11:59:00.000Z' }));
    const groups = model.groups();
    expect(groups).toHaveLength(4);
    // Chronological by creation; ties by crew id then category.
    expect(groups.map((g) => [g.crewId, g.category, g.createdAt])).toEqual([
      [CREW_A, 'CARDIO', '2026-10-10T11:59:00.000Z'],
      [CREW_A, 'CARDIO', '2026-10-10T12:00:00.000Z'],
      [CREW_A, 'THERMAL', '2026-10-10T12:00:00.000Z'],
      [CREW_B, 'CARDIO', '2026-10-10T12:00:00.000Z'],
    ]);
  });

  it('F-GND-04 groups the same creation instant written with different precision', () => {
    expect(alarmKey(alarm({ createdAt: '2026-10-10T12:00:00Z' }))).toBe(
      alarmKey(alarm({ createdAt: '2026-10-10T12:00:00.000000Z' })),
    );
  });

  it('F-GND-04 drops a duplicate packet (same APID, sequence count and packet time)', () => {
    const model = new AlertTimelineModel();
    expect(model.add(alarm())).toBe(true);
    expect(model.add(alarm())).toBe(false);
    expect(model.add(alarm({ seq: 2 }))).toBe(true);
    expect(model.groups()[0]?.transitions).toHaveLength(2);
    model.clear();
    expect(model.groups()).toEqual([]);
    expect(model.add(alarm())).toBe(true);
  });
});

const dom = useDom();

function mount(fetch = fakeFetch([]), crew: readonly CrewEntry[] = CREW) {
  const { client, ws } = startedClient(fetch);
  const time = new FakeTime(BOUNDS);
  const view = new AlertTimelineView({ client, crew, time });
  const el = dom().container();
  view.show(el);
  const items = (): HTMLLIElement[] =>
    Array.from(el.querySelectorAll<HTMLLIElement>('.caelum-alert-timeline__list > li'));
  const transitions = (li: HTMLLIElement | undefined): string[] =>
    Array.from(li?.querySelectorAll('.caelum-alarm__transitions > li') ?? []).map(
      (t) => t.textContent ?? '',
    );
  const status = (): string => el.querySelector('.caelum-alert-timeline__status')?.textContent ?? '';
  const live = (): string => el.querySelector('[aria-live="polite"]')?.textContent ?? '';
  return { client, ws, time, view, el, items, transitions, status, live };
}

describe('F-GND-04 alert timeline view', () => {
  beforeEach(() => {
    FakeWebSocket.reset();
  });

  it('F-GND-04 loads history for the time bounds and renders grouped alarms with text and glyph', async () => {
    const fetch = fakeFetch([
      opsAlarm({ state: 'confirming', at: '2026-10-10T12:00:01Z', seq: 1, escalationStage: 0, priority: 'warning' }),
      opsAlarm({ state: 'active', at: '2026-10-10T12:00:02Z', seq: 2, escalationStage: 1, priority: 'critical' }),
      opsAlarm({ crewId: CREW_B, category: 'RESPIRATORY', priority: 'observe', seq: 3 }),
    ]);
    const t = mount(fetch);
    expect(t.status()).toBe('History: loading');
    await flush();
    expect(fetch.urls).toEqual([
      'https://gateway.example/caelum/history?type=ops_alarm&start=1000&end=2000',
    ]);
    expect(t.status()).toBe('History: 3 alarm messages in the time bounds');
    const [first, second] = t.items();
    expect(t.items()).toHaveLength(2);
    expect(first?.tabIndex).toBe(0);
    expect(first?.dataset['priority']).toBe('critical');
    expect(first?.querySelector('.caelum-status__text')?.textContent).toBe('CRITICAL');
    expect(first?.querySelector('.caelum-status__glyph')?.textContent).toBe(STATUS_PRESENTATION.critical.glyph);
    expect(first?.querySelector('.caelum-status__glyph')?.getAttribute('aria-hidden')).toBe('true');
    expect(first?.querySelector('.caelum-alarm__title')?.textContent).toBe(
      ' Commander · CARDIO · created 2026-10-10T12:00:00.000Z · ACTIVE · escalation stage 1',
    );
    expect(t.transitions(first)).toEqual([
      '2026-10-10T12:00:01Z — CONFIRMING — WARNING — escalation stage 0',
      '2026-10-10T12:00:02Z — ACTIVE — CRITICAL — escalation stage 1',
    ]);
    expect(first?.getAttribute('aria-label')).toBe(
      'Commander, CARDIO, CRITICAL, ACTIVE, created 2026-10-10T12:00:00.000Z, 2 transitions',
    );
    expect(second?.querySelector('.caelum-status__text')?.textContent).toBe('OBSERVE');
  });

  it('F-GND-04 appends realtime state transitions to the group and announces them', async () => {
    const t = mount(fakeFetch([opsAlarm({ state: 'active', seq: 1 })]));
    await flush();
    t.ws.send(opsAlarm({ state: 'acknowledged', at: '2026-10-10T12:00:05Z', seq: 2, escalationStage: 2 }));
    t.ws.send(opsAlarm({ state: 'ended', at: '2026-10-10T12:00:09Z', seq: 3, escalationStage: 2 }));
    expect(t.items()).toHaveLength(1);
    expect(t.transitions(t.items()[0]).map((s) => s.split(' — ')[1])).toEqual([
      'ACTIVE',
      'ACKNOWLEDGED',
      'ENDED',
    ]);
    expect(t.live()).toBe('Commander: CARDIO CRITICAL, ENDED');
    // The same packet again (e.g. history overlap) is not duplicated.
    t.ws.send(opsAlarm({ state: 'ended', at: '2026-10-10T12:00:09Z', seq: 3, escalationStage: 2 }));
    expect(t.transitions(t.items()[0])).toHaveLength(3);
  });

  it('F-GND-04 shows an empty, labelled list when there are no alarms', async () => {
    const t = mount();
    await flush();
    expect(t.items()).toHaveLength(0);
    expect(t.el.querySelector('ol')?.getAttribute('aria-label')).toBe('No alarm messages');
    expect(t.status()).toBe('History: 0 alarm messages in the time bounds');
  });

  it('F-GND-04 refetches on bounds changes but not on realtime clock ticks', async () => {
    const fetch = fakeFetch([]);
    const t = mount(fetch);
    await flush();
    t.time.set({ start: 5_000, end: 6_000 }, true);
    await flush();
    expect(fetch.urls).toHaveLength(1);
    t.time.set({ start: 5_000, end: 6_000 }, false);
    await flush();
    expect(fetch.urls).toHaveLength(2);
    expect(fetch.urls[1]).toContain('start=5000&end=6000');
  });

  it('F-GND-04 replaces old-bounds alarms and keeps realtime alarms that arrive while loading', async () => {
    const fetch = deferredFetch();
    const t = mount(fetch);
    fetch.respond([opsAlarm({ seq: 1 })]);
    await flush();
    expect(t.items()).toHaveLength(1);

    t.time.set({ start: 5_000, end: 6_000 });
    t.ws.send(opsAlarm({ crewId: CREW_B, seq: 9 })); // arrives during the load
    fetch.respond([opsAlarm({ category: 'THERMAL', seq: 5 })]);
    await flush();
    const crews = t.items().map((li) => li.querySelector('.caelum-alarm__title')?.textContent ?? '');
    expect(crews).toHaveLength(2);
    expect(crews.some((c) => c.includes('Commander · THERMAL'))).toBe(true);
    expect(crews.some((c) => c.includes('Flight engineer · CARDIO'))).toBe(true);
    expect(crews.some((c) => c.includes('Commander · CARDIO'))).toBe(false);
  });

  it('F-GND-04 a history failure is shown and old-bounds alarms are not presented as current', async () => {
    const fetch = deferredFetch();
    const t = mount(fetch);
    fetch.respond([opsAlarm()]);
    await flush();
    expect(t.items()).toHaveLength(1);
    t.time.set({ start: 5_000, end: 6_000 });
    fetch.respond({ error: 'x' }, 503);
    await flush();
    expect(t.status()).toBe('History unavailable: history request failed: HTTP 503');
    expect(t.items()).toHaveLength(0);
  });

  it('F-GND-04 a superseded history response is ignored', async () => {
    const fetch = deferredFetch();
    const t = mount(fetch);
    t.time.set({ start: 5_000, end: 6_000 });
    fetch.respond([opsAlarm({ category: 'THERMAL' })]); // first (stale) request
    fetch.respond([]); // current request
    await flush();
    expect(t.items()).toHaveLength(0);
    expect(t.status()).toBe('History: 0 alarm messages in the time bounds');
  });

  it('R-SEC-06 F-GND-04 history items carrying physiology are dropped and counted', async () => {
    const t = mount(
      fakeFetch([
        { ...opsAlarm({ seq: 1 }), value: 31.2 },
        { ...opsAlarm({ seq: 2 }), parameter: 'core_temp' },
        { ...opsAlarm({ seq: 3 }), ruleId: 'R-TEMP-02' },
        { ...opsAlarm({ seq: 4 }), text: 'hypothermia' },
      ]),
    );
    await flush();
    expect(t.items()).toHaveLength(0);
    expect(t.client.rejectedCount).toBe(4);
    for (const leak of ['31.2', 'core_temp', 'R-TEMP-02', 'hypothermia']) {
      expect(t.el.textContent).not.toContain(leak);
    }
  });

  it('F-GND-04 renders labels as text only and unsubscribes on destroy', async () => {
    const t = mount(fakeFetch([opsAlarm()]), [{ id: CREW_A, label: '<b onclick="x()">A</b>' }]);
    await flush();
    expect(t.el.querySelector('b')).toBeNull();
    expect(t.el.textContent).toContain('<b onclick="x()">A</b>');
    t.view.destroy();
    expect(t.time.listeners.size).toBe(0);
    expect(t.el.children).toHaveLength(0);
    t.ws.send(opsAlarm({ seq: 2 })); // must not throw
  });

  it('F-GND-04 view provider handles only the alert timeline type', () => {
    const { client } = startedClient();
    const provider = createAlertTimelineViewProvider({ client, crew: CREW, time: new FakeTime(BOUNDS) });
    const o = (type: string) => ({ identifier: { namespace: NAMESPACE, key: 'k' }, type, name: 'n' });
    expect(provider.canView(o(TYPE_ALERT_TIMELINE))).toBe(true);
    expect(provider.canView(o(TYPE_CREW_BOARD))).toBe(false);
    expect(provider.view(o(TYPE_ALERT_TIMELINE))).toBeInstanceOf(AlertTimelineView);
  });
});
