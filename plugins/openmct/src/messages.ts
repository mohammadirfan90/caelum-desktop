// SPDX-License-Identifier: Apache-2.0
// Copyright ChronoStrider.
/**
 * CAELUM ground link (OPS channel, path B) message contract and strict runtime validators.
 *
 * F-GND-04 / R-SEC-06: the OPS channel carries only crew id, priority, alarm category, alarm
 * state, escalation stage, timestamps and technical health. It never carries physiology (no
 * measured value, parameter name, rule id or free text). The validators therefore accept exactly
 * the fields below: an unknown `type`, an unknown enum value, a missing field, an extra field
 * (for example `value`, `parameter`, `ruleId` or `text`), a malformed UUID or a malformed
 * timestamp rejects the whole message. Nothing is coerced, defaulted or repaired.
 *
 * Rejection reasons name only fields of this contract; they never echo an inbound value or the
 * name of an unexpected field, so a message that wrongly carries physiology cannot leak it into
 * logs or the UI through the reason text.
 */

export const PRIORITIES = ['critical', 'warning', 'observe'] as const;
export type Priority = (typeof PRIORITIES)[number];

export const CATEGORIES = ['CARDIO', 'RESPIRATORY', 'THERMAL', 'ENVIRONMENT', 'TECHNICAL'] as const;
export type Category = (typeof CATEGORIES)[number];

export const ALARM_STATES = [
  'confirming',
  'active',
  'cleared_awaiting_ack',
  'acknowledged',
  'ended',
] as const;
export type AlarmState = (typeof ALARM_STATES)[number];

export const INSTANCE_HEALTH = ['ok', 'degraded', 'fault'] as const;
export type InstanceHealth = (typeof INSTANCE_HEALTH)[number];

export const MDS_LINK_STATES = ['not_configured', 'connected', 'degraded', 'unreachable'] as const;
export type MdsLinkState = (typeof MDS_LINK_STATES)[number];

/** Largest CCSDS Space Packet APID (11 bits). */
export const MAX_APID = 2047;
/** Largest CCSDS packet sequence count (14 bits). */
export const MAX_SEQ = 16383;
/** Largest escalation stage (u8 on the wire). */
export const MAX_ESCALATION_STAGE = 255;

/** One alarm event on the OPS channel. */
export interface OpsAlarm {
  readonly type: 'ops_alarm';
  /** Crew member UUID (lowercase, hyphenated). */
  readonly crewId: string;
  readonly priority: Priority;
  readonly category: Category;
  readonly state: AlarmState;
  /** Integer 0..255. */
  readonly escalationStage: number;
  /** Alarm creation time, ISO-8601 UTC. */
  readonly createdAt: string;
  /** Packet time, ISO-8601 UTC. */
  readonly at: string;
  /** CCSDS APID, integer 0..2047. */
  readonly apid: number;
  /** CCSDS sequence count, integer 0..16383. */
  readonly seq: number;
}

/**
 * Highest active alarm for one crew member. `highestPriority`, `category` and `state` are all
 * null together when the crew member has no active alarm; that is a received fact, not a default.
 */
export interface CrewStatus {
  readonly type: 'crew_status';
  readonly crewId: string;
  readonly highestPriority: Priority | null;
  readonly category: Category | null;
  readonly state: AlarmState | null;
  readonly at: string;
  readonly apid: number;
  readonly seq: number;
}

/** Technical health of the CAELUM instance and its link to the onboard medical data system. */
export interface InstanceStatus {
  readonly type: 'instance_status';
  readonly health: InstanceHealth;
  readonly mdsLink: MdsLinkState;
  readonly at: string;
  readonly apid: number;
  readonly seq: number;
}

export type GroundMessage = OpsAlarm | CrewStatus | InstanceStatus;
export type GroundMessageType = GroundMessage['type'];

export type ValidationResult<T> =
  | { readonly ok: true; readonly message: T }
  | { readonly ok: false; readonly reason: string };

const OPS_ALARM_KEYS = [
  'type',
  'crewId',
  'priority',
  'category',
  'state',
  'escalationStage',
  'createdAt',
  'at',
  'apid',
  'seq',
] as const;
const CREW_STATUS_KEYS = [
  'type',
  'crewId',
  'highestPriority',
  'category',
  'state',
  'at',
  'apid',
  'seq',
] as const;
const INSTANCE_STATUS_KEYS = ['type', 'health', 'mdsLink', 'at', 'apid', 'seq'] as const;

/** Canonical lowercase hyphenated UUID, as emitted by the Rust `uuid` crate. */
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const NIL_UUID = '00000000-0000-0000-0000-000000000000';

/**
 * ISO-8601 UTC with a literal `Z`: `YYYY-MM-DDTHH:MM:SS[.f{1,9}]Z`. Offsets other than `Z`,
 * missing seconds, lowercase separators and leap seconds (`:60`, which JavaScript time cannot
 * represent) are rejected.
 */
const UTC_RE = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z$/;

/** True if `value` is a canonical, non-nil crew UUID. */
export function isCrewUuid(value: unknown): value is string {
  return typeof value === 'string' && UUID_RE.test(value) && value !== NIL_UUID;
}

/**
 * Parses a contract timestamp to epoch milliseconds (sub-millisecond digits truncated), or
 * returns null if it is not a valid ISO-8601 UTC timestamp. Calendar validity is checked
 * explicitly (no rollover of 30 February into March).
 */
export function parseUtc(value: unknown): number | null {
  if (typeof value !== 'string') return null;
  const m = UTC_RE.exec(value);
  if (m === null) return null;
  const [, ys, mos, ds, hs, mis, ss, frac] = m;
  const year = Number(ys);
  const month = Number(mos);
  const day = Number(ds);
  const hour = Number(hs);
  const minute = Number(mis);
  const second = Number(ss);
  if (month < 1 || month > 12) return null;
  if (day < 1 || day > daysInMonth(year, month)) return null;
  if (hour > 23 || minute > 59 || second > 59) return null;
  const ms = frac === undefined ? 0 : Number(frac.padEnd(3, '0').slice(0, 3));
  const t = Date.UTC(year, month - 1, day, hour, minute, second, ms);
  // Date.UTC maps years 0..99 to 1900..1999; reject anything that did not round-trip.
  if (new Date(t).getUTCFullYear() !== year) return null;
  return t;
}

function daysInMonth(year: number, month: number): number {
  const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  const table = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return table[month - 1] ?? 0;
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false;
  const proto: unknown = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

function isIntIn(value: unknown, min: number, max: number): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= min && value <= max;
}

function isOneOf<T extends string>(list: readonly T[], value: unknown): value is T {
  return typeof value === 'string' && (list as readonly string[]).includes(value);
}

/** Exact key set: every expected key present as an own property and no other key. */
function checkKeys(rec: Record<string, unknown>, keys: readonly string[]): string | null {
  const own = Object.keys(rec);
  if (own.length !== keys.length || own.some((k) => !keys.includes(k))) {
    // Deliberately does not name the unexpected field (it could be a parameter name).
    return 'field set does not match the OPS contract (missing or unexpected field)';
  }
  return null;
}

function fail<T>(reason: string): ValidationResult<T> {
  return { ok: false, reason };
}

function checkPacketFields(rec: Record<string, unknown>): string | null {
  if (parseUtc(rec['at']) === null) return 'at: not an ISO-8601 UTC timestamp';
  if (!isIntIn(rec['apid'], 0, MAX_APID)) return 'apid: not an integer 0..2047';
  if (!isIntIn(rec['seq'], 0, MAX_SEQ)) return 'seq: not an integer 0..16383';
  return null;
}

/** Validates an `ops_alarm` message (F-GND-04, R-SEC-06). */
export function validateOpsAlarm(input: unknown): ValidationResult<OpsAlarm> {
  if (!isPlainRecord(input)) return fail('not a JSON object');
  if (input['type'] !== 'ops_alarm') return fail('type: not ops_alarm');
  const keys = checkKeys(input, OPS_ALARM_KEYS);
  if (keys !== null) return fail(keys);
  if (!isCrewUuid(input['crewId'])) return fail('crewId: not a canonical non-nil UUID');
  if (!isOneOf(PRIORITIES, input['priority'])) return fail('priority: unknown value');
  if (!isOneOf(CATEGORIES, input['category'])) return fail('category: unknown value');
  if (!isOneOf(ALARM_STATES, input['state'])) return fail('state: unknown value');
  if (!isIntIn(input['escalationStage'], 0, MAX_ESCALATION_STAGE)) {
    return fail('escalationStage: not an integer 0..255');
  }
  if (parseUtc(input['createdAt']) === null) return fail('createdAt: not an ISO-8601 UTC timestamp');
  const packet = checkPacketFields(input);
  if (packet !== null) return fail(packet);
  return {
    ok: true,
    message: {
      type: 'ops_alarm',
      crewId: input['crewId'],
      priority: input['priority'],
      category: input['category'],
      state: input['state'],
      escalationStage: input['escalationStage'],
      createdAt: input['createdAt'] as string,
      at: input['at'] as string,
      apid: input['apid'] as number,
      seq: input['seq'] as number,
    },
  };
}

/** Validates a `crew_status` message (F-GND-04, R-SEC-06). */
export function validateCrewStatus(input: unknown): ValidationResult<CrewStatus> {
  if (!isPlainRecord(input)) return fail('not a JSON object');
  if (input['type'] !== 'crew_status') return fail('type: not crew_status');
  const keys = checkKeys(input, CREW_STATUS_KEYS);
  if (keys !== null) return fail(keys);
  if (!isCrewUuid(input['crewId'])) return fail('crewId: not a canonical non-nil UUID');
  const p = input['highestPriority'];
  const c = input['category'];
  const s = input['state'];
  if (p !== null && !isOneOf(PRIORITIES, p)) return fail('highestPriority: unknown value');
  if (c !== null && !isOneOf(CATEGORIES, c)) return fail('category: unknown value');
  if (s !== null && !isOneOf(ALARM_STATES, s)) return fail('state: unknown value');
  const nulls = [p, c, s].filter((v) => v === null).length;
  if (nulls !== 0 && nulls !== 3) {
    return fail('highestPriority, category and state must be all null or all set');
  }
  const packet = checkPacketFields(input);
  if (packet !== null) return fail(packet);
  return {
    ok: true,
    message: {
      type: 'crew_status',
      crewId: input['crewId'],
      highestPriority: p,
      category: c,
      state: s,
      at: input['at'] as string,
      apid: input['apid'] as number,
      seq: input['seq'] as number,
    },
  };
}

/** Validates an `instance_status` message (F-GND-04, R-SEC-06). */
export function validateInstanceStatus(input: unknown): ValidationResult<InstanceStatus> {
  if (!isPlainRecord(input)) return fail('not a JSON object');
  if (input['type'] !== 'instance_status') return fail('type: not instance_status');
  const keys = checkKeys(input, INSTANCE_STATUS_KEYS);
  if (keys !== null) return fail(keys);
  if (!isOneOf(INSTANCE_HEALTH, input['health'])) return fail('health: unknown value');
  if (!isOneOf(MDS_LINK_STATES, input['mdsLink'])) return fail('mdsLink: unknown value');
  const packet = checkPacketFields(input);
  if (packet !== null) return fail(packet);
  return {
    ok: true,
    message: {
      type: 'instance_status',
      health: input['health'],
      mdsLink: input['mdsLink'],
      at: input['at'] as string,
      apid: input['apid'] as number,
      seq: input['seq'] as number,
    },
  };
}

/** Validates any ground-link message by its `type`; an unknown `type` is rejected. */
export function validateGroundMessage(input: unknown): ValidationResult<GroundMessage> {
  if (!isPlainRecord(input)) return fail('not a JSON object');
  switch (input['type']) {
    case 'ops_alarm':
      return validateOpsAlarm(input);
    case 'crew_status':
      return validateCrewStatus(input);
    case 'instance_status':
      return validateInstanceStatus(input);
    default:
      return fail('type: unknown message type');
  }
}

/** Parses one WebSocket text frame as JSON and validates it. Binary frames are rejected. */
export function parseGroundFrame(data: unknown): ValidationResult<GroundMessage> {
  if (typeof data !== 'string') return fail('frame: not a text frame');
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return fail('frame: not valid JSON');
  }
  return validateGroundMessage(parsed);
}

/** Packet time of a validated message in epoch milliseconds. */
export function packetTimeMs(message: GroundMessage): number {
  // Validated messages always carry a parseable `at`; NaN would only follow a contract bug.
  return parseUtc(message.at) ?? Number.NaN;
}
