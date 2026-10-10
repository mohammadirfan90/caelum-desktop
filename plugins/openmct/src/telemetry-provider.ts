// SPDX-License-Identifier: Apache-2.0
// Copyright ChronoStrider.
/**
 * Open MCT telemetry provider and telemetry metadata for the CAELUM OPS channel (F-GND-04).
 *
 * Object types:
 * - `caelum.ops-alarm-stream`: every `ops_alarm`; history from the gateway history endpoint.
 * - `caelum.crew-status`: `crew_status` of one crew member (crew id in `domainObject.caelum`).
 * - `caelum.instance-status`: `instance_status` of the CAELUM instance.
 *
 * Datums carry only OPS-contract fields (R-SEC-06). Enumerated fields are numeric codes with
 * Open MCT `enum` metadata, so tables show the text and plots can use the code. Code 0 means
 * "no active alarm" and appears only on crew status, where the gateway sent null explicitly.
 *
 * The contract has no history endpoint for crew or instance status, so `request` for those
 * returns the latest reading received in this session if it falls inside the requested bounds,
 * and nothing otherwise. Nothing is invented to fill the range.
 *
 * Crew and instance status objects also implement the Open MCT staleness API
 * (`supportsStaleness`, `isStale`, `subscribeToStaleness`), so native Open MCT views mark a
 * reading older than `staleAfterMs`, or a missing one, as stale (SR-004, H-02).
 */

import type { GroundLinkClient } from './client.js';
import {
  ALARM_STATES,
  CATEGORIES,
  INSTANCE_HEALTH,
  MDS_LINK_STATES,
  PRIORITIES,
  packetTimeMs,
  parseUtc,
  type CrewStatus,
  type GroundMessage,
  type InstanceStatus,
  type OpsAlarm,
} from './messages.js';
import type {
  Datum,
  DomainObject,
  EnumerationEntry,
  TelemetryProvider,
  TelemetryRequestOptions,
  ValueMetadata,
} from './openmct-types.js';

export const NAMESPACE = 'caelum';
export const TYPE_OPS_ALARM_STREAM = 'caelum.ops-alarm-stream';
export const TYPE_CREW_STATUS = 'caelum.crew-status';
export const TYPE_INSTANCE_STATUS = 'caelum.instance-status';
export const TYPE_CREW_BOARD = 'caelum.crew-board';
export const TYPE_ALERT_TIMELINE = 'caelum.alert-timeline';

const TELEMETRY_TYPES: readonly string[] = [
  TYPE_OPS_ALARM_STREAM,
  TYPE_CREW_STATUS,
  TYPE_INSTANCE_STATUS,
];

/** Code 0: the gateway reported no active alarm (null fields on `crew_status`). */
export const NO_ACTIVE_ALARM_CODE = 0;

/** Priority codes ascend with urgency. */
export const PRIORITY_CODE: Readonly<Record<(typeof PRIORITIES)[number], number>> = {
  observe: 1,
  warning: 2,
  critical: 3,
};

function codes<T extends string>(list: readonly T[]): Readonly<Record<T, number>> {
  const out = {} as Record<T, number>;
  list.forEach((v, i) => {
    out[v] = i + 1;
  });
  return out;
}

export const CATEGORY_CODE = codes(CATEGORIES);
export const STATE_CODE = codes(ALARM_STATES);
export const HEALTH_CODE = codes(INSTANCE_HEALTH);
export const MDS_LINK_CODE = codes(MDS_LINK_STATES);

function enumerations<T extends string>(
  table: Readonly<Record<T, number>>,
  none?: string,
): EnumerationEntry[] {
  const entries = (Object.entries(table) as Array<[string, number]>).map(([string, value]) => ({
    value,
    string,
  }));
  if (none !== undefined) entries.unshift({ value: NO_ACTIVE_ALARM_CODE, string: none });
  return entries.sort((a, b) => a.value - b.value);
}

const UTC_VALUE: ValueMetadata = {
  key: 'utc',
  source: 'utc',
  name: 'Packet time',
  format: 'utc',
  hints: { domain: 1 },
};
const APID_VALUE: ValueMetadata = { key: 'apid', name: 'APID', format: 'number' };
const SEQ_VALUE: ValueMetadata = { key: 'seq', name: 'Sequence count', format: 'number' };
const CREW_VALUE: ValueMetadata = { key: 'crewId', name: 'Crew id', format: 'string' };

/** Telemetry metadata (`domainObject.telemetry.values`) for an object type. */
export function telemetryValuesFor(type: string): ValueMetadata[] {
  switch (type) {
    case TYPE_OPS_ALARM_STREAM:
      return [
        UTC_VALUE,
        { key: 'createdAt', name: 'Alarm created', format: 'utc' },
        CREW_VALUE,
        {
          key: 'priority',
          name: 'Priority',
          format: 'enum',
          enumerations: enumerations(PRIORITY_CODE),
          hints: { range: 1 },
        },
        {
          key: 'escalationStage',
          name: 'Escalation stage',
          format: 'number',
          hints: { range: 2 },
        },
        {
          key: 'state',
          name: 'Alarm state',
          format: 'enum',
          enumerations: enumerations(STATE_CODE),
          hints: { range: 3 },
        },
        {
          key: 'category',
          name: 'Category',
          format: 'enum',
          enumerations: enumerations(CATEGORY_CODE),
          hints: { range: 4 },
        },
        APID_VALUE,
        SEQ_VALUE,
      ];
    case TYPE_CREW_STATUS:
      return [
        UTC_VALUE,
        CREW_VALUE,
        {
          key: 'highestPriority',
          name: 'Highest priority',
          format: 'enum',
          enumerations: enumerations(PRIORITY_CODE, 'no active alarm'),
          hints: { range: 1 },
        },
        {
          key: 'state',
          name: 'Alarm state',
          format: 'enum',
          enumerations: enumerations(STATE_CODE, 'no active alarm'),
          hints: { range: 2 },
        },
        {
          key: 'category',
          name: 'Category',
          format: 'enum',
          enumerations: enumerations(CATEGORY_CODE, 'no active alarm'),
          hints: { range: 3 },
        },
        APID_VALUE,
        SEQ_VALUE,
      ];
    case TYPE_INSTANCE_STATUS:
      return [
        UTC_VALUE,
        {
          key: 'health',
          name: 'Instance health',
          format: 'enum',
          enumerations: enumerations(HEALTH_CODE),
          hints: { range: 1 },
        },
        {
          key: 'mdsLink',
          name: 'Medical data system link',
          format: 'enum',
          enumerations: enumerations(MDS_LINK_CODE),
          hints: { range: 2 },
        },
        APID_VALUE,
        SEQ_VALUE,
      ];
    default:
      return [];
  }
}

/** Converts a validated `ops_alarm` to a datum. */
export function opsAlarmDatum(m: OpsAlarm): Datum {
  return {
    utc: packetTimeMs(m),
    createdAt: parseUtc(m.createdAt) ?? Number.NaN,
    crewId: m.crewId,
    priority: PRIORITY_CODE[m.priority],
    escalationStage: m.escalationStage,
    state: STATE_CODE[m.state],
    category: CATEGORY_CODE[m.category],
    apid: m.apid,
    seq: m.seq,
  };
}

/** Converts a validated `crew_status` to a datum (null fields become code 0). */
export function crewStatusDatum(m: CrewStatus): Datum {
  return {
    utc: packetTimeMs(m),
    crewId: m.crewId,
    highestPriority:
      m.highestPriority === null ? NO_ACTIVE_ALARM_CODE : PRIORITY_CODE[m.highestPriority],
    state: m.state === null ? NO_ACTIVE_ALARM_CODE : STATE_CODE[m.state],
    category: m.category === null ? NO_ACTIVE_ALARM_CODE : CATEGORY_CODE[m.category],
    apid: m.apid,
    seq: m.seq,
  };
}

/** Converts a validated `instance_status` to a datum. */
export function instanceStatusDatum(m: InstanceStatus): Datum {
  return {
    utc: packetTimeMs(m),
    health: HEALTH_CODE[m.health],
    mdsLink: MDS_LINK_CODE[m.mdsLink],
    apid: m.apid,
    seq: m.seq,
  };
}

function isCaelumTelemetry(domainObject: DomainObject): boolean {
  return (
    domainObject.identifier.namespace === NAMESPACE && TELEMETRY_TYPES.includes(domainObject.type)
  );
}

/** Datum for `message` if it belongs to `domainObject`, otherwise null. */
function datumFor(domainObject: DomainObject, message: GroundMessage): Datum | null {
  switch (domainObject.type) {
    case TYPE_OPS_ALARM_STREAM:
      return message.type === 'ops_alarm' ? opsAlarmDatum(message) : null;
    case TYPE_CREW_STATUS:
      return message.type === 'crew_status' && message.crewId === domainObject.caelum?.crewId
        ? crewStatusDatum(message)
        : null;
    case TYPE_INSTANCE_STATUS:
      return message.type === 'instance_status' ? instanceStatusDatum(message) : null;
    default:
      return null;
  }
}

function inBounds(t: number, options: TelemetryRequestOptions | undefined): boolean {
  if (options?.start !== undefined && t < options.start) return false;
  if (options?.end !== undefined && t > options.end) return false;
  return true;
}

function isStatusObject(domainObject: DomainObject): boolean {
  return (
    domainObject.identifier.namespace === NAMESPACE &&
    (domainObject.type === TYPE_CREW_STATUS || domainObject.type === TYPE_INSTANCE_STATUS)
  );
}

/**
 * True unless the status object has a fresh reading. NO DATA counts as stale: there is no
 * current reading to rely on (H-02), so Open MCT must not present one as current.
 */
function statusIsStale(client: GroundLinkClient, domainObject: DomainObject): boolean {
  if (domainObject.type === TYPE_INSTANCE_STATUS) {
    return client.getInstanceStatus().freshness !== 'fresh';
  }
  const crewId = domainObject.caelum?.crewId;
  if (crewId === undefined) return true;
  return client.getCrewStatus(crewId).freshness !== 'fresh';
}

/** The provider with the optional Open MCT staleness methods always present. */
export type CaelumTelemetryProvider = Required<TelemetryProvider>;

export function createTelemetryProvider(client: GroundLinkClient): CaelumTelemetryProvider {
  return {
    supportsRequest: isCaelumTelemetry,
    supportsSubscribe: isCaelumTelemetry,
    supportsStaleness: isStatusObject,

    isStale(domainObject) {
      return Promise.resolve({
        isStale: statusIsStale(client, domainObject),
        utc: client.currentTimeMs(),
      });
    },

    subscribeToStaleness(domainObject, callback) {
      let last = statusIsStale(client, domainObject);
      return client.onChange(() => {
        const now = statusIsStale(client, domainObject);
        if (now === last) return;
        last = now;
        callback({ isStale: now, utc: client.currentTimeMs() });
      });
    },

    async request(domainObject, options) {
      switch (domainObject.type) {
        case TYPE_OPS_ALARM_STREAM: {
          if (options?.start === undefined || options.end === undefined) {
            throw new RangeError('CAELUM ops alarm history needs start and end bounds');
          }
          const alarms = await client.fetchAlarmHistory(options.start, options.end, options.signal);
          return alarms.map(opsAlarmDatum);
        }
        case TYPE_CREW_STATUS: {
          const crewId = domainObject.caelum?.crewId;
          if (crewId === undefined) return [];
          const reading = client.getCrewStatus(crewId);
          if (reading.freshness === 'no_data' || !inBounds(reading.atMs, options)) return [];
          return [crewStatusDatum(reading.message)];
        }
        case TYPE_INSTANCE_STATUS: {
          const reading = client.getInstanceStatus();
          if (reading.freshness === 'no_data' || !inBounds(reading.atMs, options)) return [];
          return [instanceStatusDatum(reading.message)];
        }
        default:
          return [];
      }
    },

    subscribe(domainObject, callback) {
      return client.subscribe((message) => {
        const datum = datumFor(domainObject, message);
        if (datum !== null) callback(datum);
      });
    },
  };
}
