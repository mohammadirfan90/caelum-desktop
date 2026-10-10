// SPDX-License-Identifier: Apache-2.0
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Datum, DomainObject, StalenessResponse } from '../src/openmct-types.js';
import {
  CATEGORY_CODE,
  NAMESPACE,
  NO_ACTIVE_ALARM_CODE,
  PRIORITY_CODE,
  STATE_CODE,
  TYPE_ALERT_TIMELINE,
  TYPE_CREW_BOARD,
  TYPE_CREW_STATUS,
  TYPE_INSTANCE_STATUS,
  TYPE_OPS_ALARM_STREAM,
  createTelemetryProvider,
  telemetryValuesFor,
} from '../src/telemetry-provider.js';
import {
  CREW_A,
  CREW_B,
  FakeWebSocket,
  STALE_MS,
  crewStatus,
  fakeFetch,
  instanceStatus,
  opsAlarm,
  startedClient,
} from './fixtures.js';

const T0 = Date.UTC(2026, 9, 10, 12, 0, 0);
const iso = (ms: number): string => new Date(ms).toISOString();

function obj(type: string, extra: Partial<DomainObject> = {}): DomainObject {
  return { identifier: { namespace: NAMESPACE, key: type }, type, name: type, ...extra };
}
const OPS_STREAM = obj(TYPE_OPS_ALARM_STREAM);
const CREW_A_STATUS = obj(TYPE_CREW_STATUS, { caelum: { crewId: CREW_A } });
const INSTANCE = obj(TYPE_INSTANCE_STATUS);

/** Field names that would indicate physiology on the OPS channel (R-SEC-06). */
const PHYSIOLOGY_KEYS = ['value', 'parameter', 'ruleId', 'text', 'unit', 'heartRate', 'spo2'];

describe('F-GND-04 telemetry provider', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
    FakeWebSocket.reset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('F-GND-04 supports request and subscribe only for CAELUM telemetry objects', () => {
    const { client } = startedClient();
    const provider = createTelemetryProvider(client);
    for (const o of [OPS_STREAM, CREW_A_STATUS, INSTANCE]) {
      expect(provider.supportsRequest(o)).toBe(true);
      expect(provider.supportsSubscribe(o)).toBe(true);
    }
    for (const o of [
      obj(TYPE_CREW_BOARD),
      obj(TYPE_ALERT_TIMELINE),
      { ...OPS_STREAM, identifier: { namespace: 'yamcs', key: 'x' } },
      obj('folder'),
    ]) {
      expect(provider.supportsRequest(o)).toBe(false);
      expect(provider.supportsSubscribe(o)).toBe(false);
    }
    expect(provider.supportsStaleness(CREW_A_STATUS)).toBe(true);
    expect(provider.supportsStaleness(INSTANCE)).toBe(true);
    expect(provider.supportsStaleness(OPS_STREAM)).toBe(false);
  });

  it('F-GND-04 request on the alarm stream fetches history for the bounds and maps codes', async () => {
    const fetch = fakeFetch([
      opsAlarm(),
      { ...opsAlarm({ seq: 2 }), ruleId: 'R-HR-01' }, // R-SEC-06: dropped
    ]);
    const { client } = startedClient(fetch);
    const provider = createTelemetryProvider(client);
    const data = await provider.request(OPS_STREAM, { start: 1_000, end: 2_000 });
    expect(fetch.urls).toEqual([
      'https://gateway.example/caelum/history?type=ops_alarm&start=1000&end=2000',
    ]);
    expect(data).toEqual([
      {
        utc: Date.parse('2026-10-10T12:00:01.000Z'),
        createdAt: Date.parse('2026-10-10T12:00:00.000Z'),
        crewId: CREW_A,
        priority: PRIORITY_CODE.critical,
        escalationStage: 1,
        state: STATE_CODE.active,
        category: CATEGORY_CODE.CARDIO,
        apid: 1100,
        seq: 1,
      },
    ]);
    expect(client.rejectedCount).toBe(1);
  });

  it('F-GND-04 request on the alarm stream without bounds is refused, not defaulted', async () => {
    const { client } = startedClient();
    const provider = createTelemetryProvider(client);
    await expect(provider.request(OPS_STREAM, {})).rejects.toBeInstanceOf(RangeError);
  });

  it('F-GND-04 request on crew status returns nothing before data and the latest reading in bounds', async () => {
    const { client, ws } = startedClient();
    const provider = createTelemetryProvider(client);
    expect(await provider.request(CREW_A_STATUS, { start: 0, end: T0 + 1 })).toEqual([]);
    ws.send(crewStatus({ at: iso(T0) }));
    expect(await provider.request(CREW_A_STATUS, { start: T0 - 1, end: T0 + 1 })).toEqual([
      {
        utc: T0,
        crewId: CREW_A,
        highestPriority: PRIORITY_CODE.warning,
        state: STATE_CODE.active,
        category: CATEGORY_CODE.THERMAL,
        apid: 1101,
        seq: 7,
      },
    ]);
    // Outside the bounds: nothing, not a carried-forward value.
    expect(await provider.request(CREW_A_STATUS, { start: T0 + 1, end: T0 + 5 })).toEqual([]);
    // A crew-status object without a crew id never returns another crew member's data.
    expect(await provider.request(obj(TYPE_CREW_STATUS), { start: 0, end: T0 + 1 })).toEqual([]);
  });

  it('F-GND-04 crew status with no active alarm maps to code 0, never to a priority', async () => {
    const { client, ws } = startedClient();
    const provider = createTelemetryProvider(client);
    ws.send(crewStatus({ at: iso(T0), highestPriority: null, category: null, state: null }));
    const [datum] = await provider.request(CREW_A_STATUS, { start: 0, end: T0 });
    expect(datum?.['highestPriority']).toBe(NO_ACTIVE_ALARM_CODE);
    expect(datum?.['state']).toBe(NO_ACTIVE_ALARM_CODE);
    expect(datum?.['category']).toBe(NO_ACTIVE_ALARM_CODE);
    expect(Object.values(PRIORITY_CODE)).not.toContain(NO_ACTIVE_ALARM_CODE);
  });

  it('F-GND-04 request on instance status returns the latest reading in bounds', async () => {
    const { client, ws } = startedClient();
    const provider = createTelemetryProvider(client);
    expect(await provider.request(INSTANCE, { start: 0, end: T0 })).toEqual([]);
    ws.send(instanceStatus({ at: iso(T0), health: 'degraded', mdsLink: 'unreachable' }));
    const data = await provider.request(INSTANCE, { start: 0, end: T0 });
    expect(data).toHaveLength(1);
    expect(data[0]?.['health']).toBe(2);
    expect(data[0]?.['mdsLink']).toBe(4);
  });

  it('F-GND-04 subscribe delivers only matching messages and stops after unsubscribe', () => {
    const { client, ws } = startedClient();
    const provider = createTelemetryProvider(client);
    const stream: Datum[] = [];
    const crewA: Datum[] = [];
    const instance: Datum[] = [];
    const offStream = provider.subscribe(OPS_STREAM, (d) => stream.push(d));
    provider.subscribe(CREW_A_STATUS, (d) => crewA.push(d));
    provider.subscribe(INSTANCE, (d) => instance.push(d));

    ws.send(opsAlarm());
    ws.send(crewStatus({ crewId: CREW_B }));
    ws.send(crewStatus({ crewId: CREW_A, seq: 8 }));
    ws.send(instanceStatus());
    expect(stream.map((d) => d['seq'])).toEqual([1]);
    expect(crewA.map((d) => d['seq'])).toEqual([8]);
    expect(instance.map((d) => d['seq'])).toEqual([3]);

    offStream();
    ws.send(opsAlarm({ seq: 2 }));
    expect(stream).toHaveLength(1);
  });

  it('R-SEC-06 F-GND-04 subscribe never delivers a message carrying value, parameter, ruleId or text', () => {
    const { client, ws } = startedClient();
    const provider = createTelemetryProvider(client);
    const got: Datum[] = [];
    provider.subscribe(OPS_STREAM, (d) => got.push(d));
    provider.subscribe(CREW_A_STATUS, (d) => got.push(d));
    ws.send({ ...opsAlarm(), value: 142 });
    ws.send({ ...opsAlarm(), parameter: 'heart_rate' });
    ws.send({ ...crewStatus(), ruleId: 'R-HR-01' });
    ws.send({ ...crewStatus(), text: 'tachycardia' });
    expect(got).toEqual([]);
    expect(client.rejectedCount).toBe(4);
  });

  it('R-SEC-06 F-GND-04 datums carry exactly the metadata keys and no physiology-like key', async () => {
    const { client, ws } = startedClient();
    const provider = createTelemetryProvider(client);
    const got = new Map<string, Datum>();
    provider.subscribe(OPS_STREAM, (d) => got.set(TYPE_OPS_ALARM_STREAM, d));
    provider.subscribe(CREW_A_STATUS, (d) => got.set(TYPE_CREW_STATUS, d));
    provider.subscribe(INSTANCE, (d) => got.set(TYPE_INSTANCE_STATUS, d));
    ws.send(opsAlarm());
    ws.send(crewStatus());
    ws.send(instanceStatus());
    for (const type of [TYPE_OPS_ALARM_STREAM, TYPE_CREW_STATUS, TYPE_INSTANCE_STATUS]) {
      const keys = telemetryValuesFor(type).map((v) => v.key);
      expect(Object.keys(got.get(type) ?? {}).sort()).toEqual([...keys].sort());
      for (const k of PHYSIOLOGY_KEYS) expect(keys).not.toContain(k);
      expect(telemetryValuesFor(type).filter((v) => v.hints?.domain === 1).map((v) => v.key)).toEqual(['utc']);
    }
    expect(telemetryValuesFor(TYPE_CREW_BOARD)).toEqual([]);
  });

  it('F-GND-04 SR-004 staleness API: NO DATA and old readings are stale, fresh ones are not', async () => {
    const { client, ws } = startedClient();
    const provider = createTelemetryProvider(client);
    const events: StalenessResponse[] = [];
    provider.subscribeToStaleness(CREW_A_STATUS, (r) => events.push(r));

    expect(await provider.isStale(CREW_A_STATUS)).toEqual({ isStale: true, utc: T0 });
    expect((await provider.isStale(INSTANCE)).isStale).toBe(true);
    expect((await provider.isStale(obj(TYPE_CREW_STATUS))).isStale).toBe(true);

    ws.send(crewStatus({ at: iso(T0) }));
    expect((await provider.isStale(CREW_A_STATUS)).isStale).toBe(false);
    expect(events).toEqual([{ isStale: false, utc: T0 }]);

    ws.send(crewStatus({ at: iso(T0), seq: 9 })); // no transition, no event
    expect(events).toHaveLength(1);

    vi.advanceTimersByTime(STALE_MS + 1);
    expect((await provider.isStale(CREW_A_STATUS)).isStale).toBe(true);
    expect(events).toEqual([
      { isStale: false, utc: T0 },
      { isStale: true, utc: T0 + STALE_MS + 1 },
    ]);
  });

  it('F-GND-04 staleness subscription stops after unsubscribe', () => {
    const { client, ws } = startedClient();
    const provider = createTelemetryProvider(client);
    const events: StalenessResponse[] = [];
    const off = provider.subscribeToStaleness(INSTANCE, (r) => events.push(r));
    off();
    ws.send(instanceStatus({ at: iso(T0) }));
    expect(events).toEqual([]);
  });
});
