// SPDX-License-Identifier: Apache-2.0
// Test equipment only: fixed, readable message fixtures and fakes. Never shipped.
import { GroundLinkClient, type FetchLike, type WebSocketLike } from '../src/client.js';
import type { BoundsListener, TimeApiLike, TimeBounds } from '../src/openmct-types.js';

export const CREW_A = '3f2b8c1e-7a4d-4e6f-9b0a-1c2d3e4f5a6b';
export const CREW_B = 'a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d';
export const CREW_C = 'c0ffee00-1234-4abc-8def-0123456789ab';

export function opsAlarm(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    type: 'ops_alarm',
    crewId: CREW_A,
    priority: 'critical',
    category: 'CARDIO',
    state: 'active',
    escalationStage: 1,
    createdAt: '2026-10-10T12:00:00.000Z',
    at: '2026-10-10T12:00:01.000Z',
    apid: 1100,
    seq: 1,
    ...overrides,
  };
}

export function crewStatus(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    type: 'crew_status',
    crewId: CREW_A,
    highestPriority: 'warning',
    category: 'THERMAL',
    state: 'active',
    at: '2026-10-10T12:00:00.000Z',
    apid: 1101,
    seq: 7,
    ...overrides,
  };
}

export function instanceStatus(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    type: 'instance_status',
    health: 'ok',
    mdsLink: 'connected',
    at: '2026-10-10T12:00:00.000Z',
    apid: 1102,
    seq: 3,
    ...overrides,
  };
}

/** Fake WebSocket: records instances so tests can open, message and close them. */
export class FakeWebSocket implements WebSocketLike {
  static instances: FakeWebSocket[] = [];
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { readonly data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  closed = false;
  readonly url: string;

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }

  static reset(): void {
    FakeWebSocket.instances = [];
  }

  static latest(): FakeWebSocket {
    const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1];
    if (ws === undefined) throw new Error('no FakeWebSocket created');
    return ws;
  }

  open(): void {
    this.onopen?.({});
  }

  send(message: unknown): void {
    this.onmessage?.({ data: typeof message === 'string' ? message : JSON.stringify(message) });
  }

  drop(): void {
    this.onerror?.({});
    this.onclose?.({});
  }

  close(): void {
    this.closed = true;
  }
}

/** Fake fetch returning a fixed response and recording requested URLs. */
export function fakeFetch(
  body: unknown,
  status = 200,
): FetchLike & { urls: string[] } {
  const urls: string[] = [];
  const fn = (async (url: string) => {
    urls.push(url);
    return {
      ok: status >= 200 && status < 300,
      status,
      json: async () => body,
    };
  }) as FetchLike & { urls: string[] };
  fn.urls = urls;
  return fn;
}

/** Fake fetch whose responses the test resolves one by one, in request order. */
export function deferredFetch(): FetchLike & {
  urls: string[];
  respond(body: unknown, status?: number): void;
} {
  const urls: string[] = [];
  const pending: Array<(r: { ok: boolean; status: number; json(): Promise<unknown> }) => void> =
    [];
  const fn = ((url: string) => {
    urls.push(url);
    return new Promise((resolve) => pending.push(resolve));
  }) as FetchLike & { urls: string[]; respond(body: unknown, status?: number): void };
  fn.urls = urls;
  fn.respond = (body: unknown, status = 200) => {
    const next = pending.shift();
    if (next === undefined) throw new Error('no pending fetch');
    next({ ok: status >= 200 && status < 300, status, json: async () => body });
  };
  return fn;
}

export class FakeTime implements TimeApiLike {
  bounds: TimeBounds;
  listeners = new Set<BoundsListener>();
  constructor(bounds: TimeBounds) {
    this.bounds = bounds;
  }
  getBounds(): TimeBounds {
    return this.bounds;
  }
  on(_event: 'boundsChanged', callback: BoundsListener): void {
    this.listeners.add(callback);
  }
  off(_event: 'boundsChanged', callback: BoundsListener): void {
    this.listeners.delete(callback);
  }
  set(bounds: TimeBounds, isTick = false): void {
    this.bounds = bounds;
    for (const l of this.listeners) l(bounds, isTick);
  }
}

/** Staleness limit used by the view and provider tests. */
export const STALE_MS = 10_000;

/**
 * A real client on the fake transport, started, with its socket open. Feed it with
 * `ws.send(message)`.
 */
export function startedClient(fetch: FetchLike = fakeFetch([])): {
  client: GroundLinkClient;
  ws: FakeWebSocket;
} {
  const client = new GroundLinkClient({
    baseUrl: 'https://gateway.example/caelum',
    wsUrl: 'wss://gateway.example/caelum/ops',
    staleAfterMs: STALE_MS,
    WebSocket: FakeWebSocket,
    fetch,
  });
  client.start();
  const ws = FakeWebSocket.latest();
  ws.open();
  return { client, ws };
}

/** Resolves pending promise callbacks (fetch fakes are async). */
export async function flush(): Promise<void> {
  for (let i = 0; i < 50; i += 1) await Promise.resolve();
}
