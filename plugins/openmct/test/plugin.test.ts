// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it } from 'vitest';
import CaelumPlugin, {
  ALERT_TIMELINE_KEY,
  CREW_BOARD_KEY,
  CaelumPluginConfigError,
  INSTANCE_STATUS_KEY,
  OPS_ALARMS_KEY,
  ROOT_KEY,
  crewStatusKey,
  resolveConfig,
} from '../src/index.js';
import type {
  DomainObject,
  Identifier,
  ObjectProvider,
  OpenMctLike,
  TelemetryProvider,
  TypeDefinition,
  ViewProvider,
} from '../src/openmct-types.js';
import {
  NAMESPACE,
  TYPE_ALERT_TIMELINE,
  TYPE_CREW_BOARD,
  TYPE_CREW_STATUS,
  TYPE_INSTANCE_STATUS,
  TYPE_OPS_ALARM_STREAM,
} from '../src/telemetry-provider.js';
import {
  CREW_A,
  CREW_B,
  FakeTime,
  FakeWebSocket,
  crewStatus,
  fakeFetch,
} from './fixtures.js';

/** Fake Open MCT recording every registration. */
function fakeOpenMct() {
  const types = new Map<string, TypeDefinition>();
  const roots: Identifier[] = [];
  const objectProviders = new Map<string, ObjectProvider>();
  const telemetryProviders: TelemetryProvider[] = [];
  const viewProviders: ViewProvider[] = [];
  const time = new FakeTime({ start: 0, end: 1 });
  const openmct: OpenMctLike = {
    types: { addType: (key, def) => void types.set(key, def) },
    objects: {
      addRoot: (identifier) => void roots.push(identifier),
      addProvider: (namespace, provider) => void objectProviders.set(namespace, provider),
    },
    telemetry: { addProvider: (p) => void telemetryProviders.push(p) },
    objectViews: { addProvider: (p) => void viewProviders.push(p) },
    time,
  };
  return { openmct, types, roots, objectProviders, telemetryProviders, viewProviders };
}

const VALID = {
  baseUrl: 'https://gateway.example/caelum',
  wsUrl: 'wss://gateway.example/caelum/ops',
  staleAfterMs: 15_000,
  crew: [
    { id: CREW_A, label: 'Commander' },
    { id: CREW_B, label: 'Flight engineer' },
  ],
  transport: { WebSocket: FakeWebSocket, fetch: fakeFetch([]) },
};

describe('F-GND-04 plugin install', () => {
  beforeEach(() => {
    FakeWebSocket.reset();
  });

  it('F-GND-04 registers types, root, object provider, telemetry provider and both views', () => {
    const f = fakeOpenMct();
    CaelumPlugin(VALID)(f.openmct);

    expect([...f.types.keys()].sort()).toEqual(
      [
        TYPE_ALERT_TIMELINE,
        TYPE_CREW_BOARD,
        TYPE_CREW_STATUS,
        TYPE_INSTANCE_STATUS,
        TYPE_OPS_ALARM_STREAM,
      ].sort(),
    );
    for (const def of f.types.values()) {
      expect(def.creatable).toBe(false);
      expect(def.name).toMatch(/^CAELUM /);
    }
    expect(f.roots).toEqual([{ namespace: NAMESPACE, key: ROOT_KEY }]);
    expect([...f.objectProviders.keys()]).toEqual([NAMESPACE]);
    expect(f.telemetryProviders).toHaveLength(1);
    expect(f.viewProviders.map((v) => v.key)).toEqual([
      'caelum.crew-board-view',
      'caelum.alert-timeline-view',
    ]);
    // The ground link is opened on install, to the configured URL.
    expect(FakeWebSocket.instances.map((ws) => ws.url)).toEqual([VALID.wsUrl]);
  });

  it('F-GND-04 object provider serves the root folder, fixed objects and one status object per crew member', async () => {
    const f = fakeOpenMct();
    CaelumPlugin(VALID)(f.openmct);
    const provider = f.objectProviders.get(NAMESPACE);
    const get = (key: string): Promise<DomainObject | undefined> =>
      provider?.get({ namespace: NAMESPACE, key }) ?? Promise.resolve(undefined);

    const root = await get(ROOT_KEY);
    expect(root?.type).toBe('folder');
    expect(root?.composition?.map((i) => i.key)).toEqual([
      OPS_ALARMS_KEY,
      INSTANCE_STATUS_KEY,
      CREW_BOARD_KEY,
      ALERT_TIMELINE_KEY,
      crewStatusKey(CREW_A),
      crewStatusKey(CREW_B),
    ]);
    const crewA = await get(crewStatusKey(CREW_A));
    expect(crewA?.type).toBe(TYPE_CREW_STATUS);
    expect(crewA?.name).toBe('Crew status: Commander');
    expect(crewA?.caelum).toEqual({ crewId: CREW_A });
    expect((await get(OPS_ALARMS_KEY))?.telemetry?.values.length).toBeGreaterThan(0);
    expect(await get('unknown')).toBeUndefined();
  });

  it('F-GND-04 the installed telemetry provider and views are wired to the live client', () => {
    const f = fakeOpenMct();
    CaelumPlugin(VALID)(f.openmct);
    const telemetry = f.telemetryProviders[0];
    const crewA: DomainObject = {
      identifier: { namespace: NAMESPACE, key: crewStatusKey(CREW_A) },
      type: TYPE_CREW_STATUS,
      name: 'A',
      caelum: { crewId: CREW_A },
    };
    const got: unknown[] = [];
    telemetry?.subscribe(crewA, (d) => got.push(d));
    FakeWebSocket.latest().send(crewStatus());
    expect(got).toHaveLength(1);
    const board = f.viewProviders[0];
    const timeline = f.viewProviders[1];
    const o = (type: string): DomainObject => ({ identifier: { namespace: NAMESPACE, key: 'k' }, type, name: 'n' });
    expect(board?.canView(o(TYPE_CREW_BOARD))).toBe(true);
    expect(timeline?.canView(o(TYPE_ALERT_TIMELINE))).toBe(true);
  });

  it('F-GND-04 crew is optional (crew members seen on the link are shown by UUID)', () => {
    const { crew: _crew, ...noCrew } = VALID;
    const f = fakeOpenMct();
    expect(() => CaelumPlugin(noCrew)(f.openmct)).not.toThrow();
  });

  it.each([
    ['options', undefined],
    ['baseUrl', { ...VALID, baseUrl: undefined }],
    ['baseUrl', { ...VALID, baseUrl: '' }],
    ['baseUrl', { ...VALID, baseUrl: 'gateway.example' }],
    ['baseUrl', { ...VALID, baseUrl: 'ws://gateway.example' }],
    ['wsUrl', { ...VALID, wsUrl: undefined }],
    ['wsUrl', { ...VALID, wsUrl: 'https://gateway.example/ops' }],
    ['staleAfterMs', { ...VALID, staleAfterMs: undefined }],
    ['staleAfterMs', { ...VALID, staleAfterMs: 0 }],
    ['staleAfterMs', { ...VALID, staleAfterMs: -1 }],
    ['staleAfterMs', { ...VALID, staleAfterMs: Number.POSITIVE_INFINITY }],
    ['staleAfterMs', { ...VALID, staleAfterMs: '15000' }],
    ['crew', { ...VALID, crew: 'Commander' }],
    ['crew[].id', { ...VALID, crew: [{ id: 'not-a-uuid', label: 'X' }] }],
    ['crew[].id', { ...VALID, crew: [{ id: CREW_A.toUpperCase(), label: 'X' }] }],
    ['crew[].id', { ...VALID, crew: [null] }],
    ['crew[].label', { ...VALID, crew: [{ id: CREW_A, label: ' ' }] }],
    ['crew[].id', { ...VALID, crew: [{ id: CREW_A, label: 'A' }, { id: CREW_A, label: 'B' }] }],
  ])('F-GND-04 missing or invalid %s throws before anything is registered', (field, options) => {
    const f = fakeOpenMct();
    let error: unknown;
    try {
      CaelumPlugin(options as never)(f.openmct);
    } catch (e) {
      error = e;
    }
    expect(error).toBeInstanceOf(CaelumPluginConfigError);
    expect((error as Error).message).toContain(field);
    expect(f.types.size).toBe(0);
    expect(FakeWebSocket.instances).toHaveLength(0);
  });

  it('F-GND-04 resolveConfig keeps exactly the configured values (no defaults added)', () => {
    expect(resolveConfig(VALID)).toEqual({
      baseUrl: VALID.baseUrl,
      wsUrl: VALID.wsUrl,
      staleAfterMs: 15_000,
      crew: VALID.crew,
    });
    expect(resolveConfig({ ...VALID, crew: undefined }).crew).toEqual([]);
  });
});
