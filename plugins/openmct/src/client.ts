// SPDX-License-Identifier: Apache-2.0
// Copyright ChronoStrider.
/**
 * Ground-link client for the CAELUM OPS channel (F-GND-04).
 *
 * - Realtime: one WebSocket to `wsUrl`; every text frame is parsed and strictly validated
 *   (`messages.ts`). Rejected frames are counted and dropped, never coerced (R-SEC-06).
 *   The socket reconnects with exponential back-off after any close until `stop()`.
 * - History: `GET {baseUrl}/history?type=ops_alarm&start=<ms>&end=<ms>`; each array element is
 *   validated the same way.
 * - Staleness: the latest `crew_status` per crew member and the latest `instance_status` are
 *   kept with their packet time. A reading is STALE when it is older than `staleAfterMs`
 *   (by packet time against the local clock) and NO DATA when nothing has been received.
 *   A timer fires a change event at the moment a reading becomes stale, so views update
 *   without polling. There is no default or "nominal" reading.
 *
 * The WebSocket constructor, `fetch` and the clock are injected so tests can drive them.
 */

import {
  packetTimeMs,
  parseGroundFrame,
  validateOpsAlarm,
  type CrewStatus,
  type GroundMessage,
  type InstanceStatus,
  type OpsAlarm,
} from './messages.js';

/** The part of a WebSocket the client uses. */
export interface WebSocketLike {
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { readonly data: unknown }) => void) | null;
  onclose: ((event: unknown) => void) | null;
  onerror: ((event: unknown) => void) | null;
  close(): void;
}

export type WebSocketFactory = new (url: string) => WebSocketLike;

export interface FetchResponseLike {
  readonly ok: boolean;
  readonly status: number;
  json(): Promise<unknown>;
}

export type FetchLike = (
  url: string,
  init?: { readonly signal?: AbortSignal },
) => Promise<FetchResponseLike>;

export interface BackoffOptions {
  /** First reconnect delay in milliseconds. */
  readonly initialMs: number;
  /** Upper bound for the reconnect delay in milliseconds. */
  readonly maxMs: number;
}

/** Reconnect timing (link behaviour, not data): 1 s doubling up to 30 s. */
export const DEFAULT_BACKOFF: BackoffOptions = { initialMs: 1_000, maxMs: 30_000 };

export interface GroundLinkClientOptions {
  readonly baseUrl: string;
  readonly wsUrl: string;
  readonly staleAfterMs: number;
  readonly WebSocket: WebSocketFactory;
  readonly fetch: FetchLike;
  readonly now?: () => number;
  readonly backoff?: BackoffOptions;
}

export type LinkState = 'connecting' | 'open' | 'closed';

/** A status reading: never received, or received and either fresh or stale. */
export type StatusReading<T> =
  | { readonly freshness: 'no_data' }
  | { readonly freshness: 'fresh' | 'stale'; readonly message: T; readonly atMs: number };

/** Error from the history endpoint (transport or contract failure; never message content). */
export class GroundLinkError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'GroundLinkError';
  }
}

interface Stored<T> {
  readonly message: T;
  readonly atMs: number;
}

const INSTANCE_KEY = '\u0000instance';
/** Largest setTimeout delay browsers and Node honour (2^31 - 1 ms). */
const MAX_TIMER_MS = 2_147_483_647;

export class GroundLinkClient {
  private readonly opts: GroundLinkClientOptions;
  private readonly now: () => number;
  private readonly backoff: BackoffOptions;

  private socket: WebSocketLike | null = null;
  private started = false;
  private attempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private state: LinkState = 'closed';
  private rejected = 0;

  private readonly crew = new Map<string, Stored<CrewStatus>>();
  private instance: Stored<InstanceStatus> | null = null;
  private readonly seenCrew = new Set<string>();
  private readonly staleTimers = new Map<string, ReturnType<typeof setTimeout>>();

  private readonly messageListeners = new Set<(message: GroundMessage) => void>();
  private readonly changeListeners = new Set<() => void>();

  constructor(options: GroundLinkClientOptions) {
    this.opts = options;
    this.now = options.now ?? (() => Date.now());
    this.backoff = options.backoff ?? DEFAULT_BACKOFF;
  }

  /** Opens the WebSocket (idempotent). After `stop()`, re-arms staleness timers. */
  start(): void {
    if (this.started) return;
    this.started = true;
    this.attempt = 0;
    for (const [crewId, stored] of this.crew) this.armStaleTimer(crewId, stored.atMs);
    if (this.instance !== null) this.armStaleTimer(INSTANCE_KEY, this.instance.atMs);
    this.connect();
  }

  /** Closes the WebSocket, cancels reconnects and staleness timers. */
  stop(): void {
    this.started = false;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    for (const timer of this.staleTimers.values()) clearTimeout(timer);
    this.staleTimers.clear();
    const socket = this.socket;
    this.socket = null;
    if (socket !== null) {
      socket.onopen = null;
      socket.onmessage = null;
      socket.onclose = null;
      socket.onerror = null;
      socket.close();
    }
    this.setLinkState('closed');
  }

  get linkState(): LinkState {
    return this.state;
  }

  /** Number of inbound messages rejected by validation (realtime frames and history items). */
  get rejectedCount(): number {
    return this.rejected;
  }

  /** Receives every validated realtime message. Returns an unsubscribe function. */
  subscribe(listener: (message: GroundMessage) => void): () => void {
    this.messageListeners.add(listener);
    return () => {
      this.messageListeners.delete(listener);
    };
  }

  /**
   * Notified when anything a view displays changes: a status reading, a staleness transition,
   * the link state or the rejected-message count. Returns an unsubscribe function.
   */
  onChange(listener: () => void): () => void {
    this.changeListeners.add(listener);
    return () => {
      this.changeListeners.delete(listener);
    };
  }

  /** Latest crew status for `crewId` with its freshness. */
  getCrewStatus(crewId: string): StatusReading<CrewStatus> {
    return this.reading(this.crew.get(crewId));
  }

  /** Latest instance status with its freshness. */
  getInstanceStatus(): StatusReading<InstanceStatus> {
    return this.reading(this.instance ?? undefined);
  }

  /** The client's clock (injected `now`), epoch ms. */
  currentTimeMs(): number {
    return this.now();
  }

  /** Crew ids seen in any validated message (crew_status or ops_alarm), sorted. */
  knownCrewIds(): string[] {
    return [...this.seenCrew].sort();
  }

  /**
   * Fetches `ops_alarm` history for `[start, end]` (epoch ms). Invalid items are counted as
   * rejected and dropped. Rejects with `GroundLinkError` on transport or contract failure.
   */
  async fetchAlarmHistory(start: number, end: number, signal?: AbortSignal): Promise<OpsAlarm[]> {
    if (!Number.isFinite(start) || !Number.isFinite(end) || start > end) {
      throw new RangeError('history range must be finite with start <= end');
    }
    const query = new URLSearchParams({
      type: 'ops_alarm',
      start: String(Math.floor(start)),
      end: String(Math.ceil(end)),
    });
    const url = `${this.opts.baseUrl.replace(/\/+$/, '')}/history?${query.toString()}`;
    const response = await this.opts.fetch(url, signal === undefined ? {} : { signal });
    if (!response.ok) {
      throw new GroundLinkError(`history request failed: HTTP ${response.status}`);
    }
    let body: unknown;
    try {
      body = await response.json();
    } catch {
      throw new GroundLinkError('history response is not valid JSON');
    }
    if (!Array.isArray(body)) {
      throw new GroundLinkError('history response is not a JSON array');
    }
    const alarms: OpsAlarm[] = [];
    let rejectedHere = 0;
    let newCrew = false;
    for (const item of body) {
      const result = validateOpsAlarm(item);
      if (result.ok) {
        alarms.push(result.message);
        if (!this.seenCrew.has(result.message.crewId)) {
          this.seenCrew.add(result.message.crewId);
          newCrew = true;
        }
      } else {
        rejectedHere += 1;
      }
    }
    this.rejected += rejectedHere;
    if (rejectedHere > 0 || newCrew) this.emitChange();
    return alarms;
  }

  /** Test hook and internal entry point for one inbound WebSocket frame. */
  handleFrame(data: unknown): void {
    const result = parseGroundFrame(data);
    if (!result.ok) {
      this.rejected += 1;
      this.emitChange();
      return;
    }
    const message = result.message;
    let changed = false;
    switch (message.type) {
      case 'crew_status':
        changed = this.storeCrew(message);
        break;
      case 'instance_status':
        changed = this.storeInstance(message);
        break;
      case 'ops_alarm':
        if (!this.seenCrew.has(message.crewId)) {
          this.seenCrew.add(message.crewId);
          changed = true;
        }
        break;
    }
    for (const listener of [...this.messageListeners]) {
      try {
        listener(message);
      } catch (error) {
        console.error('CAELUM Open MCT plugin: message listener failed', error);
      }
    }
    if (changed) this.emitChange();
  }

  private connect(): void {
    if (!this.started) return;
    this.reconnectTimer = null;
    this.setLinkState('connecting');
    let socket: WebSocketLike;
    try {
      socket = new this.opts.WebSocket(this.opts.wsUrl);
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;
    socket.onopen = () => {
      if (this.socket !== socket) return;
      this.attempt = 0;
      this.setLinkState('open');
    };
    socket.onmessage = (event) => {
      if (this.socket !== socket) return;
      this.handleFrame(event.data);
    };
    socket.onerror = () => {
      // A close event always follows an error; reconnection is handled there.
    };
    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.socket = null;
      this.scheduleReconnect();
    };
  }

  private scheduleReconnect(): void {
    if (!this.started) return;
    this.setLinkState('closed');
    const delay = Math.min(this.backoff.initialMs * 2 ** this.attempt, this.backoff.maxMs);
    this.attempt += 1;
    this.reconnectTimer = setTimeout(() => this.connect(), delay);
  }

  /** Delay before the next reconnect attempt, for diagnostics and tests. */
  nextReconnectDelayMs(): number {
    return Math.min(this.backoff.initialMs * 2 ** this.attempt, this.backoff.maxMs);
  }

  private storeCrew(message: CrewStatus): boolean {
    const atMs = packetTimeMs(message);
    this.seenCrew.add(message.crewId);
    const current = this.crew.get(message.crewId);
    // Out-of-order delivery (e.g. after loss of signal) never replaces a newer reading.
    if (current !== undefined && atMs < current.atMs) return false;
    this.crew.set(message.crewId, { message, atMs });
    this.armStaleTimer(message.crewId, atMs);
    return true;
  }

  private storeInstance(message: InstanceStatus): boolean {
    const atMs = packetTimeMs(message);
    if (this.instance !== null && atMs < this.instance.atMs) return false;
    this.instance = { message, atMs };
    this.armStaleTimer(INSTANCE_KEY, atMs);
    return true;
  }

  private armStaleTimer(key: string, atMs: number): void {
    const existing = this.staleTimers.get(key);
    if (existing !== undefined) clearTimeout(existing);
    this.staleTimers.delete(key);
    // Stale when age > staleAfterMs, i.e. from atMs + staleAfterMs + 1 onwards.
    const delay = atMs + this.opts.staleAfterMs + 1 - this.now();
    if (delay <= 0) return;
    // Timers longer than the platform maximum would fire at once; wait in steps instead.
    const step = Math.min(delay, MAX_TIMER_MS);
    this.staleTimers.set(
      key,
      setTimeout(() => {
        this.staleTimers.delete(key);
        if (step < delay) {
          this.armStaleTimer(key, atMs);
        } else {
          this.emitChange();
        }
      }, step),
    );
  }

  private reading<T>(stored: Stored<T> | undefined): StatusReading<T> {
    if (stored === undefined) return { freshness: 'no_data' };
    const stale = this.now() - stored.atMs > this.opts.staleAfterMs;
    return { freshness: stale ? 'stale' : 'fresh', message: stored.message, atMs: stored.atMs };
  }

  private setLinkState(next: LinkState): void {
    if (this.state === next) return;
    this.state = next;
    this.emitChange();
  }

  private emitChange(): void {
    for (const listener of [...this.changeListeners]) {
      try {
        listener();
      } catch (error) {
        console.error('CAELUM Open MCT plugin: change listener failed', error);
      }
    }
  }
}
