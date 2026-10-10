// SPDX-License-Identifier: Apache-2.0
// Copyright ChronoStrider.
/**
 * Display text and glyphs shared by the crew board and the alert timeline (F-GND-04).
 * Every status has both a text label and a distinct glyph shape, so no meaning depends on
 * colour. Glyphs are decorative (`aria-hidden`); the text carries the meaning.
 */

import type { AlarmState, Category, Priority } from './messages.js';

/** Board display status: an alarm priority, no active alarm, or a missing/old reading. */
export type DisplayStatus = Priority | 'none' | 'no_data' | 'stale';

export interface StatusPresentation {
  readonly glyph: string;
  readonly text: string;
}

export const STATUS_PRESENTATION: Readonly<Record<DisplayStatus, StatusPresentation>> = {
  critical: { glyph: '▲', text: 'CRITICAL' }, // ▲ triangle
  warning: { glyph: '◆', text: 'WARNING' }, // ◆ diamond
  observe: { glyph: '●', text: 'OBSERVE' }, // ● disc
  none: { glyph: '—', text: 'NO ACTIVE ALARM' }, // — dash
  no_data: { glyph: '∅', text: 'NO DATA' }, // ∅ empty set
  stale: { glyph: '◌', text: 'STALE' }, // ◌ dotted circle
};

export const STATE_TEXT: Readonly<Record<AlarmState, string>> = {
  confirming: 'CONFIRMING',
  active: 'ACTIVE',
  cleared_awaiting_ack: 'CLEARED, AWAITING ACK',
  acknowledged: 'ACKNOWLEDGED',
  ended: 'ENDED',
};

export const CATEGORY_TEXT: Readonly<Record<Category, string>> = {
  CARDIO: 'CARDIO',
  RESPIRATORY: 'RESPIRATORY',
  THERMAL: 'THERMAL',
  ENVIRONMENT: 'ENVIRONMENT',
  TECHNICAL: 'TECHNICAL',
};

export interface CrewEntry {
  readonly id: string;
  readonly label: string;
}

/** Configured display name for a crew id, or the UUID itself when none is configured. */
export function crewLabel(crew: readonly CrewEntry[], crewId: string): string {
  return crew.find((c) => c.id === crewId)?.label ?? crewId;
}

/** Builds `<span glyph aria-hidden> <span text>` using textContent only. */
export function statusBadge(doc: Document, status: DisplayStatus): HTMLSpanElement {
  const p = STATUS_PRESENTATION[status];
  const badge = doc.createElement('span');
  badge.className = `caelum-status caelum-status--${status}`;
  badge.dataset['status'] = status;
  const glyph = doc.createElement('span');
  glyph.className = 'caelum-status__glyph';
  glyph.setAttribute('aria-hidden', 'true');
  glyph.textContent = p.glyph;
  const text = doc.createElement('span');
  text.className = 'caelum-status__text';
  text.textContent = p.text;
  badge.append(glyph, ' ', text);
  return badge;
}

/** Visually hidden but announced (for aria-live regions). */
export const SR_ONLY_STYLE =
  'position:absolute;width:1px;height:1px;margin:-1px;padding:0;overflow:hidden;' +
  'clip:rect(0 0 0 0);white-space:nowrap;border:0;';
