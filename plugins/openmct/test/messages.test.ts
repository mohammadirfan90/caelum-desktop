// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import {
  parseGroundFrame,
  parseUtc,
  validateCrewStatus,
  validateGroundMessage,
  validateInstanceStatus,
  validateOpsAlarm,
} from '../src/messages.js';
import { CREW_A, crewStatus, instanceStatus, opsAlarm } from './fixtures.js';

describe('F-GND-04 OPS message validators accept the contract', () => {
  it('F-GND-04 accepts a well-formed ops_alarm for every enum value', () => {
    for (const priority of ['critical', 'warning', 'observe']) {
      for (const category of ['CARDIO', 'RESPIRATORY', 'THERMAL', 'ENVIRONMENT', 'TECHNICAL']) {
        for (const state of ['confirming', 'active', 'cleared_awaiting_ack', 'acknowledged', 'ended']) {
          const r = validateOpsAlarm(opsAlarm({ priority, category, state }));
          expect(r.ok).toBe(true);
        }
      }
    }
  });

  it('F-GND-04 accepts boundary integers and fractional-second timestamps', () => {
    for (const [escalationStage, apid, seq] of [
      [0, 0, 0],
      [255, 2047, 16383],
    ]) {
      expect(validateOpsAlarm(opsAlarm({ escalationStage, apid, seq })).ok).toBe(true);
    }
    expect(validateOpsAlarm(opsAlarm({ at: '2026-10-10T12:00:01Z' })).ok).toBe(true);
    expect(validateOpsAlarm(opsAlarm({ at: '2026-10-10T12:00:01.123456789Z' })).ok).toBe(true);
    expect(validateOpsAlarm(opsAlarm({ at: '2028-02-29T00:00:00Z' })).ok).toBe(true);
  });

  it('F-GND-04 accepts crew_status with an active alarm and with all-null (no active alarm)', () => {
    expect(validateCrewStatus(crewStatus()).ok).toBe(true);
    const none = validateCrewStatus(
      crewStatus({ highestPriority: null, category: null, state: null }),
    );
    expect(none.ok).toBe(true);
    if (none.ok) expect(none.message.highestPriority).toBeNull();
  });

  it('F-GND-04 accepts instance_status for every health and MDS link value', () => {
    for (const health of ['ok', 'degraded', 'fault']) {
      for (const mdsLink of ['not_configured', 'connected', 'degraded', 'unreachable']) {
        expect(validateInstanceStatus(instanceStatus({ health, mdsLink })).ok).toBe(true);
      }
    }
  });

  it('F-GND-04 dispatches by type and returns an exact copy without extra fields', () => {
    const r = validateGroundMessage(opsAlarm());
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.message).toEqual(opsAlarm());
    expect(validateGroundMessage(crewStatus()).ok).toBe(true);
    expect(validateGroundMessage(instanceStatus()).ok).toBe(true);
  });
});

describe('R-SEC-06 OPS validators reject physiology and malformed messages', () => {
  it.each(['value', 'parameter', 'ruleId', 'text', 'heartRate', 'note', '__proto__'])(
    'R-SEC-06 F-GND-04 rejects ops_alarm carrying extra field %s',
    (field) => {
      const raw = JSON.parse(JSON.stringify(opsAlarm()).replace(/}$/, `,"${field}":72}`)) as unknown;
      const r = validateOpsAlarm(raw);
      expect(r.ok).toBe(false);
    },
  );

  it.each(['value', 'parameter', 'ruleId', 'text'])(
    'R-SEC-06 F-GND-04 rejects crew_status and instance_status carrying %s',
    (field) => {
      expect(validateCrewStatus({ ...crewStatus(), [field]: 'SpO2 88 %' }).ok).toBe(false);
      expect(validateInstanceStatus({ ...instanceStatus(), [field]: 'x' }).ok).toBe(false);
    },
  );

  it.each(['value', 'parameter', 'ruleId', 'text'])(
    'R-SEC-06 F-GND-04 validateGroundMessage and parseGroundFrame reject %s on every message type',
    (field) => {
      for (const base of [opsAlarm(), crewStatus(), instanceStatus()]) {
        const msg = { ...base, [field]: field === 'value' ? 96 : 'x' };
        expect(validateGroundMessage(msg).ok).toBe(false);
        expect(parseGroundFrame(JSON.stringify(msg)).ok).toBe(false);
      }
    },
  );

  it('R-SEC-06 F-GND-04 rejects a physiology field even when its value is null or nested', () => {
    expect(validateOpsAlarm({ ...opsAlarm(), value: null }).ok).toBe(false);
    expect(validateOpsAlarm({ ...opsAlarm(), text: { en: 'fever' } }).ok).toBe(false);
  });

  it('R-SEC-06 rejection reasons never echo the unexpected field name or value', () => {
    const r = validateOpsAlarm({ ...opsAlarm(), heartRateBpm: 142 });
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.reason).not.toContain('heartRate');
      expect(r.reason).not.toContain('142');
    }
  });

  it('F-GND-04 rejects unknown type and non-object input', () => {
    expect(validateGroundMessage({ ...opsAlarm(), type: 'vitals' }).ok).toBe(false);
    expect(validateGroundMessage(null).ok).toBe(false);
    expect(validateGroundMessage([opsAlarm()]).ok).toBe(false);
    expect(validateGroundMessage('ops_alarm').ok).toBe(false);
    expect(validateOpsAlarm(crewStatus()).ok).toBe(false);
  });

  it.each([
    ['priority', 'high'],
    ['priority', 'CRITICAL'],
    ['priority', null],
    ['category', 'cardio'],
    ['category', 'NEURO'],
    ['state', 'resolved'],
    ['state', null],
  ])('F-GND-04 rejects ops_alarm with unknown enum %s=%s', (field, value) => {
    expect(validateOpsAlarm(opsAlarm({ [field]: value })).ok).toBe(false);
  });

  it.each([
    ['escalationStage', -1],
    ['escalationStage', 256],
    ['escalationStage', 1.5],
    ['escalationStage', '1'],
    ['apid', 2048],
    ['apid', -1],
    ['seq', 16384],
    ['seq', Number.NaN],
  ])('F-GND-04 rejects out-of-range or non-integer %s=%s', (field, value) => {
    expect(validateOpsAlarm(opsAlarm({ [field]: value })).ok).toBe(false);
  });

  it.each([
    'not-a-uuid',
    '3F2B8C1E-7A4D-4E6F-9B0A-1C2D3E4F5A6B',
    '3f2b8c1e7a4d4e6f9b0a1c2d3e4f5a6b',
    '{3f2b8c1e-7a4d-4e6f-9b0a-1c2d3e4f5a6b}',
    '00000000-0000-0000-0000-000000000000',
    42,
  ])('F-GND-04 rejects malformed crew UUID %s', (crewId) => {
    expect(validateOpsAlarm(opsAlarm({ crewId })).ok).toBe(false);
    expect(validateCrewStatus(crewStatus({ crewId })).ok).toBe(false);
  });

  it.each([
    '2026-10-10T12:00:01+00:00',
    '2026-10-10T12:00:01',
    '2026-10-10 12:00:01Z',
    '2026-10-10T12:00Z',
    '2026-02-30T00:00:00Z',
    '2027-02-29T00:00:00Z',
    '2026-13-01T00:00:00Z',
    '2026-10-10T24:00:00Z',
    '2026-12-31T23:59:60Z',
    '2026-10-10T12:00:01.Z',
    '2026-10-10t12:00:01z',
    1760097601000,
  ])('F-GND-04 rejects malformed timestamp %s', (at) => {
    expect(validateOpsAlarm(opsAlarm({ at })).ok).toBe(false);
    expect(validateOpsAlarm(opsAlarm({ createdAt: at })).ok).toBe(false);
  });

  it('F-GND-04 rejects missing fields', () => {
    const { escalationStage: _omit, ...missing } = opsAlarm();
    expect(validateOpsAlarm(missing).ok).toBe(false);
  });

  it('F-GND-04 rejects crew_status with partially null alarm fields (no coercion)', () => {
    expect(validateCrewStatus(crewStatus({ highestPriority: null })).ok).toBe(false);
    expect(validateCrewStatus(crewStatus({ category: null, state: null })).ok).toBe(false);
  });

  it('F-GND-04 rejects instance_status with unknown health or link', () => {
    expect(validateInstanceStatus(instanceStatus({ health: 'nominal' })).ok).toBe(false);
    expect(validateInstanceStatus(instanceStatus({ mdsLink: 'up' })).ok).toBe(false);
  });

  it('F-GND-04 parseGroundFrame rejects binary frames and invalid JSON', () => {
    expect(parseGroundFrame(new ArrayBuffer(4)).ok).toBe(false);
    expect(parseGroundFrame('{"type":').ok).toBe(false);
    expect(parseGroundFrame(JSON.stringify(opsAlarm())).ok).toBe(true);
  });

  it('F-GND-04 parseUtc truncates sub-millisecond digits and rejects two-digit-year rollover', () => {
    expect(parseUtc('2026-10-10T12:00:01.1239Z')).toBe(Date.UTC(2026, 9, 10, 12, 0, 1, 123));
    // Date.UTC would silently map year 0050 to 1950; the parser refuses instead of coercing.
    expect(parseUtc('0050-01-01T00:00:00Z')).toBeNull();
  });

  it('F-GND-04 validated message keeps the crew UUID exactly as received', () => {
    const r = validateCrewStatus(crewStatus());
    expect(r.ok && r.message.crewId).toBe(CREW_A);
  });
});
