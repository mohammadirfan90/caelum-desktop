// SPDX-License-Identifier: Apache-2.0
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GroundLinkClient, GroundLinkError } from '../src/client.js';
import type { GroundMessage } from '../src/messages.js';
import {
  CREW_A,
  CREW_B,
  FakeWebSocket,
  crewStatus,
  fakeFetch,
  instanceStatus,
  opsAlarm,
} from './fixtures.js';

const T0 = Date.UTC(2026, 9, 10, 12, 0, 0);

function makeClient(fetch = fakeFetch([])): GroundLinkClient {
  return new GroundLinkClient({
    baseUrl: 'https://gateway.example/caelum/',
    wsUrl: 'wss://gateway.example/caelum/ops',
    staleAfterMs: 10_000,
    WebSocket: FakeWebSocket,
    fetch,
    backoff: { initialMs: 1_000, maxMs: 8_000 },
  });
}

describe('F-GND-04 ground-link client', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
    FakeWebSocket.reset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('F-GND-04 connects, receives validated messages and reports link state', () => {
    const client = makeClient();
    const received: GroundMessage[] = [];
    client.subscribe((m) => received.push(m));
    client.start();
    expect(client.linkState).toBe('connecting');
    const ws = FakeWebSocket.latest();
    expect(ws.url).toBe('wss://gateway.example/caelum/ops');
    ws.open();
    expect(client.linkState).toBe('open');
    ws.send(opsAlarm());
    ws.send(crewStatus());
    ws.send(instanceStatus());
    expect(received.map((m) => m.type)).toEqual(['ops_alarm', 'crew_status', 'instance_status']);
    expect(client.rejectedCount).toBe(0);
  });

  it('R-SEC-06 F-GND-04 counts and drops a message carrying physiology, never delivering it', () => {
    const client = makeClient();
    const received: GroundMessage[] = [];
    client.subscribe((m) => received.push(m));
    client.start();
    const ws = FakeWebSocket.latest();
    ws.open();
    ws.send({ ...opsAlarm(), value: 38.9 });
    ws.send({ ...crewStatus(), parameter: 'core_temp' });
    ws.send({ ...opsAlarm(), ruleId: 'R-HR-01' });
    ws.send({ ...opsAlarm(), text: 'tachycardia' });
    ws.send('not json');
    expect(received).toEqual([]);
    expect(client.rejectedCount).toBe(5);
    expect(client.getCrewStatus(CREW_A).freshness).toBe('no_data');
  });

  it('F-GND-04 reconnects with exponential back-off capped at the maximum', () => {
    const client = makeClient();
    client.start();
    expect(FakeWebSocket.instances).toHaveLength(1);
    FakeWebSocket.latest().drop();
    expect(client.linkState).toBe('closed');
    vi.advanceTimersByTime(999);
    expect(FakeWebSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeWebSocket.instances).toHaveLength(2);
    FakeWebSocket.latest().drop(); // next delay 2 s
    vi.advanceTimersByTime(2_000);
    expect(FakeWebSocket.instances).toHaveLength(3);
    FakeWebSocket.latest().drop(); // 4 s
    vi.advanceTimersByTime(4_000);
    FakeWebSocket.latest().drop(); // 8 s (cap)
    vi.advanceTimersByTime(8_000);
    expect(FakeWebSocket.instances).toHaveLength(5);
    FakeWebSocket.latest().drop(); // still 8 s
    expect(client.nextReconnectDelayMs()).toBe(8_000);
    vi.advanceTimersByTime(8_000);
    expect(FakeWebSocket.instances).toHaveLength(6);
  });

  it('F-GND-04 resets back-off after a successful open', () => {
    const client = makeClient();
    client.start();
    FakeWebSocket.latest().drop();
    vi.advanceTimersByTime(1_000);
    FakeWebSocket.latest().drop();
    vi.advanceTimersByTime(2_000);
    FakeWebSocket.latest().open();
    expect(client.linkState).toBe('open');
    FakeWebSocket.latest().drop();
    expect(client.nextReconnectDelayMs()).toBe(2_000); // first retry already counted
    vi.advanceTimersByTime(1_000);
    expect(FakeWebSocket.instances).toHaveLength(4);
  });

  it('F-GND-04 reconnects when the WebSocket constructor throws', () => {
    let calls = 0;
    class ThrowingSocket extends FakeWebSocket {
      constructor(url: string) {
        calls += 1;
        if (calls === 1) throw new Error('refused');
        super(url);
      }
    }
    const client = new GroundLinkClient({
      baseUrl: 'https://g.example',
      wsUrl: 'wss://g.example/ops',
      staleAfterMs: 1_000,
      WebSocket: ThrowingSocket,
      fetch: fakeFetch([]),
    });
    client.start();
    expect(client.linkState).toBe('closed');
    vi.advanceTimersByTime(1_000);
    expect(calls).toBe(2);
    expect(client.linkState).toBe('connecting');
  });

  it('F-GND-04 stop() closes the socket and cancels reconnects', () => {
    const client = makeClient();
    client.start();
    const ws = FakeWebSocket.latest();
    client.stop();
    expect(ws.closed).toBe(true);
    vi.advanceTimersByTime(60_000);
    expect(FakeWebSocket.instances).toHaveLength(1);
    expect(client.linkState).toBe('closed');
  });

  it('F-GND-04 crew status is NO DATA until received, fresh, then STALE after the limit', () => {
    const client = makeClient();
    let changes = 0;
    client.onChange(() => {
      changes += 1;
    });
    client.start();
    FakeWebSocket.latest().open();
    expect(client.getCrewStatus(CREW_A).freshness).toBe('no_data');
    FakeWebSocket.latest().send(crewStatus({ at: new Date(T0).toISOString() }));
    expect(client.getCrewStatus(CREW_A).freshness).toBe('fresh');
    const before = changes;
    vi.advanceTimersByTime(10_000);
    expect(client.getCrewStatus(CREW_A).freshness).toBe('fresh'); // age == limit is not stale
    vi.advanceTimersByTime(1);
    expect(client.getCrewStatus(CREW_A).freshness).toBe('stale');
    expect(changes).toBe(before + 1); // staleness transition fired a change event
  });

  it('F-GND-04 a status already older than the limit is STALE on arrival', () => {
    const client = makeClient();
    client.start();
    FakeWebSocket.latest().send(crewStatus({ at: new Date(T0 - 60_000).toISOString() }));
    expect(client.getCrewStatus(CREW_A).freshness).toBe('stale');
  });

  it('F-GND-04 an older out-of-order status never replaces a newer one', () => {
    const client = makeClient();
    client.start();
    const ws = FakeWebSocket.latest();
    ws.send(crewStatus({ at: new Date(T0).toISOString(), highestPriority: 'critical' }));
    ws.send(crewStatus({ at: new Date(T0 - 1_000).toISOString(), highestPriority: 'observe' }));
    const r = client.getCrewStatus(CREW_A);
    expect(r.freshness !== 'no_data' && r.message.highestPriority).toBe('critical');
  });

  it('F-GND-04 tracks instance status staleness and known crew ids', () => {
    const client = makeClient();
    client.start();
    const ws = FakeWebSocket.latest();
    expect(client.getInstanceStatus().freshness).toBe('no_data');
    ws.send(instanceStatus({ at: new Date(T0).toISOString() }));
    ws.send(opsAlarm({ crewId: CREW_B }));
    expect(client.getInstanceStatus().freshness).toBe('fresh');
    vi.advanceTimersByTime(10_001);
    expect(client.getInstanceStatus().freshness).toBe('stale');
    expect(client.knownCrewIds()).toEqual([CREW_B]);
  });

  it('F-GND-04 fetches history with the contract URL and validates each item', async () => {
    const fetch = fakeFetch([opsAlarm(), { ...opsAlarm({ seq: 2 }), value: 120 }, crewStatus()]);
    const client = makeClient(fetch);
    const alarms = await client.fetchAlarmHistory(1000.4, 2000.6);
    expect(fetch.urls).toEqual([
      'https://gateway.example/caelum/history?type=ops_alarm&start=1000&end=2001',
    ]);
    expect(alarms).toHaveLength(1);
    expect(client.rejectedCount).toBe(2);
  });

  it('F-GND-04 history reveals new crew ids with a change event', async () => {
    const client = makeClient(fakeFetch([opsAlarm({ crewId: CREW_B })]));
    let changes = 0;
    client.onChange(() => {
      changes += 1;
    });
    await client.fetchAlarmHistory(0, 1);
    expect(client.knownCrewIds()).toEqual([CREW_B]);
    expect(changes).toBe(1);
    await client.fetchAlarmHistory(0, 1); // nothing new: no event
    expect(changes).toBe(1);
  });

  it('F-GND-04 stop() then start() re-arms the staleness notification', () => {
    const client = makeClient();
    client.start();
    FakeWebSocket.latest().send(crewStatus({ at: new Date(T0).toISOString() }));
    client.stop();
    client.start();
    let changes = 0;
    client.onChange(() => {
      changes += 1;
    });
    vi.advanceTimersByTime(10_001);
    expect(client.getCrewStatus(CREW_A).freshness).toBe('stale');
    expect(changes).toBe(1);
  });

  it('F-GND-04 a staleness limit beyond the timer maximum does not fire early', () => {
    const long = 3_000_000_000; // > 2^31 - 1 ms
    const client = new GroundLinkClient({
      baseUrl: 'https://g.example',
      wsUrl: 'wss://g.example/ops',
      staleAfterMs: long,
      WebSocket: FakeWebSocket,
      fetch: fakeFetch([]),
    });
    client.start();
    FakeWebSocket.latest().send(instanceStatus({ at: new Date(T0).toISOString() }));
    let changes = 0;
    client.onChange(() => {
      changes += 1;
    });
    vi.advanceTimersByTime(2_147_483_647);
    expect(changes).toBe(0);
    expect(client.getInstanceStatus().freshness).toBe('fresh');
    vi.advanceTimersByTime(long - 2_147_483_647 + 1);
    expect(changes).toBe(1);
    expect(client.getInstanceStatus().freshness).toBe('stale');
  });

  it('F-GND-04 history failures reject with GroundLinkError and bad ranges with RangeError', async () => {
    await expect(makeClient(fakeFetch([], 503)).fetchAlarmHistory(0, 1)).rejects.toBeInstanceOf(
      GroundLinkError,
    );
    await expect(makeClient(fakeFetch({ items: [] })).fetchAlarmHistory(0, 1)).rejects.toThrow(
      'not a JSON array',
    );
    await expect(makeClient().fetchAlarmHistory(2, 1)).rejects.toBeInstanceOf(RangeError);
    await expect(makeClient().fetchAlarmHistory(Number.NaN, 1)).rejects.toBeInstanceOf(RangeError);
  });
});
