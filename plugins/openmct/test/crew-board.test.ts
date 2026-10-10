// SPDX-License-Identifier: Apache-2.0
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  CrewBoardView,
  createCrewBoardViewProvider,
  crewBoardRows,
  instanceStatusText,
} from '../src/crew-board.js';
import { STATUS_PRESENTATION, type CrewEntry } from '../src/labels.js';
import { NAMESPACE, TYPE_ALERT_TIMELINE, TYPE_CREW_BOARD } from '../src/telemetry-provider.js';
import { useDom } from './dom.js';
import {
  CREW_A,
  CREW_B,
  CREW_C,
  FakeWebSocket,
  STALE_MS,
  crewStatus,
  instanceStatus,
  startedClient,
} from './fixtures.js';

const T0 = Date.UTC(2026, 9, 10, 12, 0, 0);
const iso = (ms: number): string => new Date(ms).toISOString();
const CREW: CrewEntry[] = [
  { id: CREW_A, label: 'Commander' },
  { id: CREW_B, label: 'Flight engineer' },
];

const dom = useDom();

function mount(crew: readonly CrewEntry[] = CREW) {
  const { client, ws } = startedClient();
  const view = new CrewBoardView({ client, crew });
  const el = dom().container();
  view.show(el);
  const row = (crewId: string): HTMLTableRowElement => {
    const tr = el.querySelector<HTMLTableRowElement>(`tr[data-crew-id="${crewId}"]`);
    if (tr === null) throw new Error(`no row for ${crewId}`);
    return tr;
  };
  const cells = (crewId: string): string[] =>
    Array.from(row(crewId).cells).map((c) => c.textContent ?? '');
  const glyph = (crewId: string): Element | null => row(crewId).querySelector('.caelum-status__glyph');
  const statusText = (crewId: string): string =>
    row(crewId).querySelector('.caelum-status__text')?.textContent ?? '';
  const live = (kind: 'polite' | 'assertive'): string =>
    el.querySelector(`[aria-live="${kind}"]`)?.textContent ?? '';
  return { client, ws, view, el, row, cells, glyph, statusText, live };
}

describe('F-GND-04 crew status board', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
    FakeWebSocket.reset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('F-GND-04 every display status has its own text and glyph shape (no colour-only meaning)', () => {
    const entries = Object.values(STATUS_PRESENTATION);
    expect(new Set(entries.map((p) => p.glyph)).size).toBe(entries.length);
    expect(new Set(entries.map((p) => p.text)).size).toBe(entries.length);
    for (const p of entries) {
      expect(p.glyph.trim()).not.toBe('');
      expect(p.text.trim()).not.toBe('');
    }
  });

  it('F-GND-04 SR-004 shows NO DATA with text and glyph before anything is received', () => {
    const b = mount();
    expect(b.statusText(CREW_A)).toBe('NO DATA');
    expect(b.glyph(CREW_A)?.textContent).toBe(STATUS_PRESENTATION.no_data.glyph);
    expect(b.glyph(CREW_A)?.getAttribute('aria-hidden')).toBe('true');
    expect(b.cells(CREW_A)).toEqual(['Commander', `${STATUS_PRESENTATION.no_data.glyph} NO DATA`, '—', '—', '—']);
    expect(b.row(CREW_A).getAttribute('aria-label')).toBe('Commander, NO DATA');
    expect(b.el.querySelector('.caelum-crew-board__instance')?.textContent).toBe(
      'CAELUM instance: NO DATA',
    );
  });

  it('F-GND-04 shows the highest priority with text, glyph, category, state and packet time', () => {
    const b = mount();
    b.ws.send(crewStatus({ crewId: CREW_A, highestPriority: 'critical', category: 'CARDIO', state: 'active', at: iso(T0) }));
    b.ws.send(crewStatus({ crewId: CREW_B, highestPriority: 'observe', category: 'ENVIRONMENT', state: 'acknowledged', at: iso(T0) }));
    expect(b.cells(CREW_A)).toEqual([
      'Commander',
      `${STATUS_PRESENTATION.critical.glyph} CRITICAL`,
      'CARDIO',
      'ACTIVE',
      iso(T0),
    ]);
    expect(b.row(CREW_A).dataset['status']).toBe('critical');
    expect(b.statusText(CREW_B)).toBe('OBSERVE');
    expect(b.glyph(CREW_B)?.textContent).toBe(STATUS_PRESENTATION.observe.glyph);
    expect(b.cells(CREW_B)[3]).toBe('ACKNOWLEDGED');
  });

  it('F-GND-04 shows NO ACTIVE ALARM only when the gateway reported null fields', () => {
    const b = mount();
    b.ws.send(crewStatus({ highestPriority: null, category: null, state: null, at: iso(T0) }));
    expect(b.statusText(CREW_A)).toBe('NO ACTIVE ALARM');
    expect(b.cells(CREW_A).slice(2, 4)).toEqual(['—', '—']);
  });

  it('F-GND-04 SR-004 H-02 a reading older than staleAfterMs shows STALE and hides the old priority', () => {
    const b = mount();
    b.ws.send(crewStatus({ highestPriority: null, category: null, state: null, at: iso(T0) }));
    vi.advanceTimersByTime(STALE_MS + 1);
    expect(b.statusText(CREW_A)).toBe('STALE');
    expect(b.glyph(CREW_A)?.textContent).toBe(STATUS_PRESENTATION.stale.glyph);
    expect(b.cells(CREW_A)).toEqual(['Commander', `${STATUS_PRESENTATION.stale.glyph} STALE`, '—', '—', iso(T0)]);
    expect(b.row(CREW_A).textContent).not.toContain('NO ACTIVE ALARM');
    expect(b.row(CREW_A).getAttribute('aria-label')).toBe(`Commander, STALE, last update ${iso(T0)}`);
  });

  it('F-GND-04 shows instance health, and STALE without the old values', () => {
    const b = mount();
    b.ws.send(instanceStatus({ health: 'degraded', mdsLink: 'unreachable', at: iso(T0) }));
    const line = (): string => b.el.querySelector('.caelum-crew-board__instance')?.textContent ?? '';
    expect(line()).toBe(
      `CAELUM instance: health DEGRADED · Medical data system link UNREACHABLE · at ${iso(T0)}`,
    );
    vi.advanceTimersByTime(STALE_MS + 1);
    expect(line()).toBe(`CAELUM instance: STALE (last update ${iso(T0)})`);
    expect(instanceStatusText(b.client)).toBe(line());
  });

  it('F-GND-04 shows link state and the rejected-message count', () => {
    const b = mount();
    const link = (): string => b.el.querySelector('.caelum-crew-board__link')?.textContent ?? '';
    expect(link()).toBe('Ground link: CONNECTED · Rejected messages: 0');
    b.ws.send({ ...crewStatus(), value: 39.2 }); // R-SEC-06: rejected
    expect(link()).toBe('Ground link: CONNECTED · Rejected messages: 1');
    expect(b.statusText(CREW_A)).toBe('NO DATA');
    b.ws.drop();
    expect(link()).toBe('Ground link: DISCONNECTED · Rejected messages: 1');
  });

  it('F-GND-04 adds rows for crew ids seen on the link but not configured, labelled by UUID', () => {
    const b = mount();
    b.ws.send(crewStatus({ crewId: CREW_C, at: iso(T0) }));
    const rows = Array.from(b.el.querySelectorAll('tbody tr')).map((r) => (r as HTMLElement).dataset['crewId']);
    expect(rows).toEqual([CREW_A, CREW_B, CREW_C]);
    expect(b.cells(CREW_C)[0]).toBe(CREW_C);
    expect(crewBoardRows({ client: b.client, crew: CREW }).map((r) => r.status)).toEqual([
      'no_data',
      'no_data',
      'warning',
    ]);
  });

  it('F-GND-04 rows are keyboard focusable and Arrow/Home/End move focus between rows', () => {
    const b = mount([...CREW, { id: CREW_C, label: 'Mission specialist' }]);
    const rows = Array.from(b.el.querySelectorAll<HTMLTableRowElement>('tbody tr'));
    expect(rows).toHaveLength(3);
    for (const r of rows) expect(r.tabIndex).toBe(0);
    const doc = dom().document;
    const KeyboardEventImpl = dom().window.KeyboardEvent;
    const key = (target: HTMLElement, k: string): boolean =>
      target.dispatchEvent(new KeyboardEventImpl('keydown', { key: k, bubbles: true, cancelable: true }));

    rows[0]?.focus();
    expect(doc.activeElement).toBe(rows[0]);
    expect(key(rows[0] as HTMLElement, 'ArrowDown')).toBe(false); // default prevented
    expect(doc.activeElement).toBe(rows[1]);
    key(rows[1] as HTMLElement, 'End');
    expect(doc.activeElement).toBe(rows[2]);
    key(rows[2] as HTMLElement, 'ArrowDown');
    expect(doc.activeElement).toBe(rows[2]);
    key(rows[2] as HTMLElement, 'ArrowUp');
    expect(doc.activeElement).toBe(rows[1]);
    key(rows[1] as HTMLElement, 'Home');
    expect(doc.activeElement).toBe(rows[0]);
    expect(key(rows[0] as HTMLElement, 'a')).toBe(true); // other keys untouched
  });

  it('F-GND-04 keeps focus on a row while it updates', () => {
    const b = mount();
    b.row(CREW_A).focus();
    b.ws.send(crewStatus({ highestPriority: 'critical', at: iso(T0) }));
    expect(dom().document.activeElement).toBe(b.row(CREW_A));
  });

  it('F-GND-04 announces changes in aria-live regions: critical assertive, others polite', () => {
    const b = mount();
    expect(b.el.querySelector('[aria-live="polite"]')?.getAttribute('aria-atomic')).toBe('true');
    expect(b.live('polite')).toBe('');
    expect(b.live('assertive')).toBe('');
    b.ws.send(crewStatus({ crewId: CREW_B, highestPriority: 'warning', category: 'THERMAL', state: 'active', at: iso(T0) }));
    expect(b.live('polite')).toBe(`Flight engineer, WARNING, category THERMAL, state ACTIVE, at ${iso(T0)}`);
    b.ws.send(crewStatus({ crewId: CREW_A, highestPriority: 'critical', category: 'CARDIO', state: 'confirming', at: iso(T0) }));
    expect(b.live('assertive')).toBe(`Commander, CRITICAL, category CARDIO, state CONFIRMING, at ${iso(T0)}`);
    // A newer packet with the same meaning is not re-announced.
    b.live('polite');
    b.ws.send(crewStatus({ crewId: CREW_B, highestPriority: 'warning', category: 'THERMAL', state: 'active', at: iso(T0 + 1_000), seq: 8 }));
    expect(b.live('polite')).toBe(`Flight engineer, WARNING, category THERMAL, state ACTIVE, at ${iso(T0)}`);
  });

  it('F-GND-04 renders crew labels as text only (no HTML injection)', () => {
    const b = mount([{ id: CREW_A, label: '<img src=x onerror="alert(1)">' }]);
    expect(b.el.querySelector('img')).toBeNull();
    expect(b.cells(CREW_A)[0]).toBe('<img src=x onerror="alert(1)">');
  });

  it('F-GND-04 destroy removes the view and stops listening', () => {
    const b = mount();
    b.view.destroy();
    expect(b.el.children).toHaveLength(0);
    b.ws.send(crewStatus({ at: iso(T0) })); // must not throw or re-render
    expect(b.el.children).toHaveLength(0);
  });

  it('F-GND-04 view provider handles only the crew board type', () => {
    const { client } = startedClient();
    const provider = createCrewBoardViewProvider({ client, crew: CREW });
    const o = (type: string) => ({ identifier: { namespace: NAMESPACE, key: 'k' }, type, name: 'n' });
    expect(provider.canView(o(TYPE_CREW_BOARD))).toBe(true);
    expect(provider.canView(o(TYPE_ALERT_TIMELINE))).toBe(false);
    expect(provider.view(o(TYPE_CREW_BOARD))).toBeInstanceOf(CrewBoardView);
  });
});
