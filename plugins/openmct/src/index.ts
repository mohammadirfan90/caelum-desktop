// SPDX-License-Identifier: Apache-2.0
// Copyright ChronoStrider.
/**
 * CAELUM Open MCT plugin (F-GND-04): telemetry provider, crew status board and alert timeline
 * for the OPS channel of the CAELUM ground link (R-SEC-06: no physiology).
 *
 * ```ts
 * openmct.install(CaelumPlugin({ baseUrl, wsUrl, staleAfterMs, crew }));
 * ```
 *
 * Required options have no defaults: a missing or invalid one throws `CaelumPluginConfigError`
 * when `CaelumPlugin()` is called, before anything is registered (no fabricated configuration).
 */

import {
  GroundLinkClient,
  type BackoffOptions,
  type FetchLike,
  type WebSocketFactory,
} from './client.js';
import { createAlertTimelineViewProvider } from './alert-timeline.js';
import { createCrewBoardViewProvider } from './crew-board.js';
import type { CrewEntry } from './labels.js';
import { isCrewUuid } from './messages.js';
import type { DomainObject, Identifier, OpenMctLike } from './openmct-types.js';
import {
  NAMESPACE,
  TYPE_ALERT_TIMELINE,
  TYPE_CREW_BOARD,
  TYPE_CREW_STATUS,
  TYPE_INSTANCE_STATUS,
  TYPE_OPS_ALARM_STREAM,
  createTelemetryProvider,
  telemetryValuesFor,
} from './telemetry-provider.js';

export interface CaelumPluginOptions {
  /** Gateway HTTP base URL (http: or https:), used for `GET {baseUrl}/history`. */
  readonly baseUrl: string;
  /** Gateway WebSocket URL (ws: or wss:) streaming OPS messages. */
  readonly wsUrl: string;
  /** A crew or instance status older than this (ms, by packet time) is shown as STALE. */
  readonly staleAfterMs: number;
  /** Display names for crew UUIDs; crew without an entry are shown by UUID. */
  readonly crew?: ReadonlyArray<{ readonly id: string; readonly label: string }>;
  /** Transport injection for embedding and tests; defaults to the browser globals. */
  readonly transport?: {
    readonly WebSocket?: WebSocketFactory;
    readonly fetch?: FetchLike;
    readonly backoff?: BackoffOptions;
  };
}

export class CaelumPluginConfigError extends Error {
  constructor(message: string) {
    super(`CAELUM Open MCT plugin configuration: ${message}`);
    this.name = 'CaelumPluginConfigError';
  }
}

export const ROOT_KEY = 'root';
export const OPS_ALARMS_KEY = 'ops-alarms';
export const INSTANCE_STATUS_KEY = 'instance-status';
export const CREW_BOARD_KEY = 'crew-board';
export const ALERT_TIMELINE_KEY = 'alert-timeline';
export const crewStatusKey = (crewId: string): string => `crew-status.${crewId}`;

function id(key: string): Identifier {
  return { namespace: NAMESPACE, key };
}

function checkUrl(name: string, value: unknown, protocols: readonly string[]): string {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new CaelumPluginConfigError(`${name} is required (no default)`);
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new CaelumPluginConfigError(`${name} is not a valid absolute URL`);
  }
  if (!protocols.includes(url.protocol)) {
    throw new CaelumPluginConfigError(`${name} must use ${protocols.join(' or ')}`);
  }
  return value;
}

interface ResolvedConfig {
  readonly baseUrl: string;
  readonly wsUrl: string;
  readonly staleAfterMs: number;
  readonly crew: readonly CrewEntry[];
}

/** Validates options; throws `CaelumPluginConfigError` on anything missing or malformed. */
export function resolveConfig(options: unknown): ResolvedConfig {
  if (typeof options !== 'object' || options === null) {
    throw new CaelumPluginConfigError('options object is required');
  }
  const o = options as Record<string, unknown>;
  const baseUrl = checkUrl('baseUrl', o['baseUrl'], ['http:', 'https:']);
  const wsUrl = checkUrl('wsUrl', o['wsUrl'], ['ws:', 'wss:']);
  const stale = o['staleAfterMs'];
  if (typeof stale !== 'number' || !Number.isFinite(stale) || stale <= 0) {
    throw new CaelumPluginConfigError('staleAfterMs is required and must be a positive number');
  }
  const crew: CrewEntry[] = [];
  const rawCrew = o['crew'];
  if (rawCrew !== undefined) {
    if (!Array.isArray(rawCrew)) throw new CaelumPluginConfigError('crew must be an array');
    for (const entry of rawCrew as unknown[]) {
      const e = entry as Record<string, unknown> | null;
      const crewId = e?.['id'];
      const label = e?.['label'];
      if (!isCrewUuid(crewId)) {
        throw new CaelumPluginConfigError('crew[].id must be a lowercase canonical crew UUID');
      }
      if (typeof label !== 'string' || label.trim() === '') {
        throw new CaelumPluginConfigError('crew[].label must be a non-empty string');
      }
      if (crew.some((c) => c.id === crewId)) {
        throw new CaelumPluginConfigError('crew[].id values must be unique');
      }
      crew.push({ id: crewId, label });
    }
  }
  return { baseUrl, wsUrl, staleAfterMs: stale, crew };
}

/** The domain objects provided under the `caelum` namespace, keyed by identifier key. */
export function buildObjects(crew: readonly CrewEntry[]): Map<string, DomainObject> {
  const location = `${NAMESPACE}:${ROOT_KEY}`;
  const objects: DomainObject[] = [
    {
      identifier: id(OPS_ALARMS_KEY),
      type: TYPE_OPS_ALARM_STREAM,
      name: 'OPS alarm events',
      location,
      telemetry: { values: telemetryValuesFor(TYPE_OPS_ALARM_STREAM) },
    },
    {
      identifier: id(INSTANCE_STATUS_KEY),
      type: TYPE_INSTANCE_STATUS,
      name: 'CAELUM instance status',
      location,
      telemetry: { values: telemetryValuesFor(TYPE_INSTANCE_STATUS) },
    },
    { identifier: id(CREW_BOARD_KEY), type: TYPE_CREW_BOARD, name: 'Crew status board', location },
    {
      identifier: id(ALERT_TIMELINE_KEY),
      type: TYPE_ALERT_TIMELINE,
      name: 'Alert timeline',
      location,
    },
    ...crew.map(
      (c): DomainObject => ({
        identifier: id(crewStatusKey(c.id)),
        type: TYPE_CREW_STATUS,
        name: `Crew status: ${c.label}`,
        location,
        telemetry: { values: telemetryValuesFor(TYPE_CREW_STATUS) },
        caelum: { crewId: c.id },
      }),
    ),
  ];
  const root: DomainObject = {
    identifier: id(ROOT_KEY),
    type: 'folder',
    name: 'CAELUM',
    location: 'ROOT',
    composition: objects.map((o) => o.identifier),
  };
  return new Map([root, ...objects].map((o) => [o.identifier.key, o]));
}

/** Creates the Open MCT plugin. Throws `CaelumPluginConfigError` on invalid options. */
export default function CaelumPlugin(options: CaelumPluginOptions): (openmct: OpenMctLike) => void {
  const config = resolveConfig(options);
  const WebSocketImpl =
    options.transport?.WebSocket ??
    // The DOM WebSocket satisfies WebSocketLike structurally except for handler `this` typing.
    (globalThis.WebSocket as unknown as WebSocketFactory | undefined);
  const fetchImpl =
    options.transport?.fetch ??
    (typeof globalThis.fetch === 'function'
      ? (globalThis.fetch.bind(globalThis) as unknown as FetchLike)
      : undefined);
  if (WebSocketImpl === undefined) throw new CaelumPluginConfigError('no WebSocket available');
  if (fetchImpl === undefined) throw new CaelumPluginConfigError('no fetch available');

  return function install(openmct: OpenMctLike): void {
    const client = new GroundLinkClient({
      baseUrl: config.baseUrl,
      wsUrl: config.wsUrl,
      staleAfterMs: config.staleAfterMs,
      WebSocket: WebSocketImpl,
      fetch: fetchImpl,
      ...(options.transport?.backoff === undefined ? {} : { backoff: options.transport.backoff }),
    });
    const objects = buildObjects(config.crew);

    openmct.types.addType(TYPE_OPS_ALARM_STREAM, {
      name: 'CAELUM OPS Alarm Stream',
      description: 'Alarm events on the CAELUM OPS channel (no physiology).',
      cssClass: 'icon-telemetry',
      creatable: false,
    });
    openmct.types.addType(TYPE_CREW_STATUS, {
      name: 'CAELUM Crew Status',
      description: 'Highest active alarm of one crew member (OPS channel).',
      cssClass: 'icon-person',
      creatable: false,
    });
    openmct.types.addType(TYPE_INSTANCE_STATUS, {
      name: 'CAELUM Instance Status',
      description: 'Technical health of the CAELUM instance and its medical data system link.',
      cssClass: 'icon-telemetry',
      creatable: false,
    });
    openmct.types.addType(TYPE_CREW_BOARD, {
      name: 'CAELUM Crew Status Board',
      description: 'One row per crew member with alarm priority, NO DATA and STALE states.',
      cssClass: 'icon-tabular-realtime',
      creatable: false,
    });
    openmct.types.addType(TYPE_ALERT_TIMELINE, {
      name: 'CAELUM Alert Timeline',
      description: 'OPS alarm events grouped by alarm with state transitions.',
      cssClass: 'icon-timeline',
      creatable: false,
    });

    openmct.objects.addProvider(NAMESPACE, {
      get: (identifier) => Promise.resolve(objects.get(identifier.key)),
    });
    openmct.objects.addRoot(id(ROOT_KEY));
    openmct.telemetry.addProvider(createTelemetryProvider(client));
    openmct.objectViews.addProvider(createCrewBoardViewProvider({ client, crew: config.crew }));
    openmct.objectViews.addProvider(
      createAlertTimelineViewProvider({ client, crew: config.crew, time: openmct.time }),
    );

    client.start();
  };
}

export { GroundLinkClient, GroundLinkError } from './client.js';
export type { BackoffOptions, FetchLike, LinkState, WebSocketFactory, WebSocketLike } from './client.js';
export * from './messages.js';
export type { OpenMctLike } from './openmct-types.js';
