// SPDX-License-Identifier: Apache-2.0
// Copyright ChronoStrider.
/**
 * Crew status board view for Open MCT object type `caelum.crew-board` (F-GND-04).
 *
 * One row per crew member: configured crew first (configuration order), then any other crew id
 * seen on the ground link (sorted, labelled by UUID). Each row shows the highest active alarm
 * priority as text AND a glyph shape, the alarm category and state, and the packet time.
 * - NO DATA: nothing received for that crew member.
 * - STALE: the latest status is older than `staleAfterMs`; the old priority is not shown, so an
 *   old "no active alarm" can never read as current reassurance (H-02).
 * Above the table: ground-link state, rejected-message count and CAELUM instance health (with
 * the same NO DATA / STALE rules).
 * Rows are keyboard focusable (Tab, plus Arrow/Home/End between rows). Status changes are
 * announced through aria-live regions (critical assertively). All text uses textContent.
 */

import type { GroundLinkClient } from './client.js';
import {
  CATEGORY_TEXT,
  SR_ONLY_STYLE,
  STATE_TEXT,
  STATUS_PRESENTATION,
  crewLabel,
  statusBadge,
  type CrewEntry,
  type DisplayStatus,
} from './labels.js';
import type { DomainObject, View, ViewProvider } from './openmct-types.js';
import { TYPE_CREW_BOARD } from './telemetry-provider.js';

export interface CrewBoardDeps {
  readonly client: GroundLinkClient;
  readonly crew: readonly CrewEntry[];
}

/** What one board row shows, derived from the client's reading. */
export interface CrewRowModel {
  readonly crewId: string;
  readonly label: string;
  readonly status: DisplayStatus;
  readonly category: string;
  readonly state: string;
  readonly packetTime: string;
}

const EMPTY = '—';

/** Board rows in display order (pure; used by the view and by tests). */
export function crewBoardRows(deps: CrewBoardDeps): CrewRowModel[] {
  const ids = deps.crew.map((c) => c.id);
  for (const id of deps.client.knownCrewIds()) if (!ids.includes(id)) ids.push(id);
  return ids.map((crewId) => {
    const label = crewLabel(deps.crew, crewId);
    const reading = deps.client.getCrewStatus(crewId);
    if (reading.freshness === 'no_data') {
      return { crewId, label, status: 'no_data', category: EMPTY, state: EMPTY, packetTime: EMPTY };
    }
    if (reading.freshness === 'stale') {
      return {
        crewId,
        label,
        status: 'stale',
        category: EMPTY,
        state: EMPTY,
        packetTime: reading.message.at,
      };
    }
    const m = reading.message;
    return {
      crewId,
      label,
      status: m.highestPriority ?? 'none',
      category: m.category === null ? EMPTY : CATEGORY_TEXT[m.category],
      state: m.state === null ? EMPTY : STATE_TEXT[m.state],
      packetTime: m.at,
    };
  });
}

function rowSummary(row: CrewRowModel): string {
  const parts = [row.label, STATUS_PRESENTATION[row.status].text];
  if (row.category !== EMPTY) parts.push(`category ${row.category}`);
  if (row.state !== EMPTY) parts.push(`state ${row.state}`);
  if (row.packetTime !== EMPTY) {
    parts.push(row.status === 'stale' ? `last update ${row.packetTime}` : `at ${row.packetTime}`);
  }
  return parts.join(', ');
}

const LINK_TEXT = { connecting: 'CONNECTING', open: 'CONNECTED', closed: 'DISCONNECTED' } as const;
const HEALTH_TEXT = { ok: 'OK', degraded: 'DEGRADED', fault: 'FAULT' } as const;
const MDS_LINK_TEXT = {
  not_configured: 'NOT CONFIGURED',
  connected: 'CONNECTED',
  degraded: 'DEGRADED',
  unreachable: 'UNREACHABLE',
} as const;

/**
 * One line of CAELUM instance technical health. A stale reading shows only its age, never the
 * old health values (same rule as the crew rows).
 */
export function instanceStatusText(client: GroundLinkClient): string {
  const reading = client.getInstanceStatus();
  if (reading.freshness === 'no_data') return 'CAELUM instance: NO DATA';
  if (reading.freshness === 'stale') {
    return `CAELUM instance: STALE (last update ${reading.message.at})`;
  }
  const m = reading.message;
  return (
    `CAELUM instance: health ${HEALTH_TEXT[m.health]}` +
    ` · Medical data system link ${MDS_LINK_TEXT[m.mdsLink]} · at ${m.at}`
  );
}

export class CrewBoardView implements View {
  private readonly deps: CrewBoardDeps;
  private root: HTMLElement | null = null;
  private tbody: HTMLTableSectionElement | null = null;
  private linkLine: HTMLElement | null = null;
  private instanceLine: HTMLElement | null = null;
  private politeRegion: HTMLElement | null = null;
  private assertiveRegion: HTMLElement | null = null;
  private readonly rows = new Map<string, HTMLTableRowElement>();
  private readonly lastKey = new Map<string, string>();
  private unsubscribe: (() => void) | null = null;

  constructor(deps: CrewBoardDeps) {
    this.deps = deps;
  }

  show(element: HTMLElement): void {
    const doc = element.ownerDocument;
    const root = doc.createElement('section');
    root.className = 'caelum-crew-board';
    root.setAttribute('aria-label', 'CAELUM crew status board');

    const link = doc.createElement('p');
    link.className = 'caelum-crew-board__link';
    this.linkLine = link;
    const instance = doc.createElement('p');
    instance.className = 'caelum-crew-board__instance';
    this.instanceLine = instance;

    const table = doc.createElement('table');
    table.className = 'caelum-crew-board__table';
    const caption = doc.createElement('caption');
    caption.textContent = 'Crew status (OPS channel: priority, category and alarm state only)';
    const thead = doc.createElement('thead');
    const headRow = doc.createElement('tr');
    for (const heading of ['Crew', 'Status', 'Category', 'Alarm state', 'Packet time (UTC)']) {
      const th = doc.createElement('th');
      th.scope = 'col';
      th.textContent = heading;
      headRow.append(th);
    }
    thead.append(headRow);
    const tbody = doc.createElement('tbody');
    tbody.addEventListener('keydown', (event) => this.onKeyDown(event));
    table.append(caption, thead, tbody);
    this.tbody = tbody;

    const polite = doc.createElement('div');
    polite.setAttribute('aria-live', 'polite');
    polite.setAttribute('aria-atomic', 'true');
    polite.className = 'caelum-live caelum-live--polite';
    polite.setAttribute('style', SR_ONLY_STYLE);
    const assertive = doc.createElement('div');
    assertive.setAttribute('aria-live', 'assertive');
    assertive.setAttribute('aria-atomic', 'true');
    assertive.className = 'caelum-live caelum-live--assertive';
    assertive.setAttribute('style', SR_ONLY_STYLE);
    this.politeRegion = polite;
    this.assertiveRegion = assertive;

    root.append(link, instance, table, polite, assertive);
    element.append(root);
    this.root = root;

    this.render(false);
    this.unsubscribe = this.deps.client.onChange(() => this.render(true));
  }

  destroy(): void {
    this.unsubscribe?.();
    this.unsubscribe = null;
    this.root?.remove();
    this.root = null;
    this.tbody = null;
    this.rows.clear();
    this.lastKey.clear();
  }

  /** Updates rows in place (keeps keyboard focus) and announces changed rows. */
  private render(announce: boolean): void {
    const tbody = this.tbody;
    if (tbody === null || this.linkLine === null) return;
    const doc = tbody.ownerDocument;
    const client = this.deps.client;
    this.linkLine.textContent =
      `Ground link: ${LINK_TEXT[client.linkState]}` +
      ` · Rejected messages: ${client.rejectedCount}`;
    if (this.instanceLine !== null) this.instanceLine.textContent = instanceStatusText(client);

    const polite: string[] = [];
    const assertive: string[] = [];
    for (const model of crewBoardRows(this.deps)) {
      let tr = this.rows.get(model.crewId);
      if (tr === undefined) {
        tr = doc.createElement('tr');
        tr.tabIndex = 0;
        tr.className = 'caelum-crew-board__row';
        tr.dataset['crewId'] = model.crewId;
        for (let i = 0; i < 5; i += 1) tr.append(doc.createElement('td'));
        tbody.append(tr);
        this.rows.set(model.crewId, tr);
      }
      const [crewCell, statusCell, categoryCell, stateCell, timeCell] = Array.from(tr.cells);
      if (!crewCell || !statusCell || !categoryCell || !stateCell || !timeCell) continue;
      crewCell.textContent = model.label;
      statusCell.replaceChildren(statusBadge(doc, model.status));
      categoryCell.textContent = model.category;
      stateCell.textContent = model.state;
      timeCell.textContent = model.packetTime;
      tr.dataset['status'] = model.status;
      const summary = rowSummary(model);
      tr.setAttribute('aria-label', summary);

      // Announce when what the row means changes, not on every packet-time refresh.
      const key = `${model.status}|${model.category}|${model.state}`;
      const changed = this.lastKey.get(model.crewId) !== key;
      this.lastKey.set(model.crewId, key);
      if (announce && changed) {
        (model.status === 'critical' ? assertive : polite).push(summary);
      }
    }
    if (polite.length > 0 && this.politeRegion) this.politeRegion.textContent = polite.join('. ');
    if (assertive.length > 0 && this.assertiveRegion) {
      this.assertiveRegion.textContent = assertive.join('. ');
    }
  }

  private onKeyDown(event: KeyboardEvent): void {
    const tbody = this.tbody;
    if (tbody === null) return;
    const rows = Array.from(tbody.rows);
    const index = rows.findIndex((r) => r === event.target);
    if (index < 0) return;
    let next: number;
    switch (event.key) {
      case 'ArrowDown':
        next = Math.min(index + 1, rows.length - 1);
        break;
      case 'ArrowUp':
        next = Math.max(index - 1, 0);
        break;
      case 'Home':
        next = 0;
        break;
      case 'End':
        next = rows.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    rows[next]?.focus();
  }
}

export function createCrewBoardViewProvider(deps: CrewBoardDeps): ViewProvider {
  return {
    key: 'caelum.crew-board-view',
    name: 'CAELUM Crew Status Board',
    cssClass: 'icon-tabular-realtime',
    canView: (domainObject: DomainObject) => domainObject.type === TYPE_CREW_BOARD,
    view: () => new CrewBoardView(deps),
  };
}
