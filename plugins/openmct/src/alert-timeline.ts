// SPDX-License-Identifier: Apache-2.0
// Copyright ChronoStrider.
/**
 * Alert timeline view for Open MCT object type `caelum.alert-timeline` (F-GND-04).
 *
 * Shows `ops_alarm` messages chronologically, grouped by alarm. The OPS channel carries no
 * alarm id (R-SEC-06 allow-list), so an alarm is identified by crew id + creation time +
 * category; two alarms of the same category for the same crew member created in the same
 * millisecond would be merged (documented limitation). Each group lists its state transitions
 * with packet time, priority and escalation stage.
 *
 * History is fetched for the Open MCT time bounds and refetched when the bounds change (not on
 * realtime clock ticks); realtime messages are appended as they arrive. Duplicates (the same
 * packet from history and realtime) are dropped by APID + sequence count + packet time.
 */

import { GroundLinkError, type GroundLinkClient } from './client.js';
import {
  CATEGORY_TEXT,
  SR_ONLY_STYLE,
  STATE_TEXT,
  STATUS_PRESENTATION,
  crewLabel,
  statusBadge,
  type CrewEntry,
} from './labels.js';
import {
  packetTimeMs,
  parseUtc,
  type AlarmState,
  type Category,
  type OpsAlarm,
  type Priority,
} from './messages.js';
import type {
  BoundsListener,
  DomainObject,
  TimeApiLike,
  TimeBounds,
  View,
  ViewProvider,
} from './openmct-types.js';
import { TYPE_ALERT_TIMELINE } from './telemetry-provider.js';

export interface AlarmTransition {
  readonly at: string;
  readonly atMs: number;
  readonly state: AlarmState;
  readonly priority: Priority;
  readonly escalationStage: number;
  readonly apid: number;
  readonly seq: number;
}

export interface AlarmGroup {
  readonly key: string;
  readonly crewId: string;
  readonly category: Category;
  readonly createdAt: string;
  readonly createdAtMs: number;
  /** Sorted by packet time, then sequence count. */
  readonly transitions: readonly AlarmTransition[];
}

/** Grouping key: crew id + creation instant + category (no alarm id on OPS). */
export function alarmKey(m: OpsAlarm): string {
  return `${m.crewId}|${parseUtc(m.createdAt) ?? m.createdAt}|${m.category}`;
}

interface MutableGroup {
  key: string;
  crewId: string;
  category: Category;
  createdAt: string;
  createdAtMs: number;
  transitions: AlarmTransition[];
}

/** Pure grouping model (no DOM). */
export class AlertTimelineModel {
  private readonly groupsByKey = new Map<string, MutableGroup>();
  private readonly seen = new Set<string>();

  /** Adds a message; returns false if the same packet was already added. */
  add(m: OpsAlarm): boolean {
    const packetKey = `${m.apid}|${m.seq}|${m.at}`;
    if (this.seen.has(packetKey)) return false;
    this.seen.add(packetKey);
    const key = alarmKey(m);
    let group = this.groupsByKey.get(key);
    if (group === undefined) {
      group = {
        key,
        crewId: m.crewId,
        category: m.category,
        createdAt: m.createdAt,
        createdAtMs: parseUtc(m.createdAt) ?? Number.NaN,
        transitions: [],
      };
      this.groupsByKey.set(key, group);
    }
    group.transitions.push({
      at: m.at,
      atMs: packetTimeMs(m),
      state: m.state,
      priority: m.priority,
      escalationStage: m.escalationStage,
      apid: m.apid,
      seq: m.seq,
    });
    group.transitions.sort((a, b) => a.atMs - b.atMs || a.seq - b.seq);
    return true;
  }

  clear(): void {
    this.groupsByKey.clear();
    this.seen.clear();
  }

  /** Groups in chronological order of alarm creation (ties: crew id, category). */
  groups(): AlarmGroup[] {
    return [...this.groupsByKey.values()].sort(
      (a, b) =>
        a.createdAtMs - b.createdAtMs ||
        a.crewId.localeCompare(b.crewId) ||
        a.category.localeCompare(b.category),
    );
  }
}

export interface AlertTimelineDeps {
  readonly client: GroundLinkClient;
  readonly crew: readonly CrewEntry[];
  readonly time: TimeApiLike;
}

function transitionText(t: AlarmTransition): string {
  return (
    `${t.at} — ${STATE_TEXT[t.state]} — ${STATUS_PRESENTATION[t.priority].text}` +
    ` — escalation stage ${t.escalationStage}`
  );
}

export class AlertTimelineView implements View {
  private readonly deps: AlertTimelineDeps;
  private readonly model = new AlertTimelineModel();
  private root: HTMLElement | null = null;
  private list: HTMLOListElement | null = null;
  private statusLine: HTMLElement | null = null;
  private liveRegion: HTMLElement | null = null;
  private readonly items = new Map<string, HTMLLIElement>();
  private unsubscribe: (() => void) | null = null;
  private abort: AbortController | null = null;
  private generation = 0;
  /** Realtime alarms received while a history load is in flight (null when idle). */
  private pendingRealtime: OpsAlarm[] | null = null;
  private readonly onBounds: BoundsListener = (bounds, isTick) => {
    if (!isTick) void this.load(bounds);
  };

  constructor(deps: AlertTimelineDeps) {
    this.deps = deps;
  }

  show(element: HTMLElement): void {
    const doc = element.ownerDocument;
    const root = doc.createElement('section');
    root.className = 'caelum-alert-timeline';
    root.setAttribute('aria-label', 'CAELUM alert timeline');
    const status = doc.createElement('p');
    status.className = 'caelum-alert-timeline__status';
    const list = doc.createElement('ol');
    list.className = 'caelum-alert-timeline__list';
    const live = doc.createElement('div');
    live.setAttribute('aria-live', 'polite');
    live.setAttribute('aria-atomic', 'true');
    live.className = 'caelum-live caelum-live--polite';
    live.setAttribute('style', SR_ONLY_STYLE);
    root.append(status, list, live);
    element.append(root);
    this.root = root;
    this.list = list;
    this.statusLine = status;
    this.liveRegion = live;

    this.unsubscribe = this.deps.client.subscribe((message) => {
      if (message.type !== 'ops_alarm') return;
      this.pendingRealtime?.push(message);
      if (this.model.add(message)) {
        this.render();
        if (this.liveRegion) {
          const label = crewLabel(this.deps.crew, message.crewId);
          this.liveRegion.textContent =
            `${label}: ${CATEGORY_TEXT[message.category]} ` +
            `${STATUS_PRESENTATION[message.priority].text}, ${STATE_TEXT[message.state]}`;
        }
      }
    });
    this.deps.time.on('boundsChanged', this.onBounds);
    void this.load(this.deps.time.getBounds());
  }

  destroy(): void {
    this.deps.time.off('boundsChanged', this.onBounds);
    this.unsubscribe?.();
    this.unsubscribe = null;
    this.abort?.abort();
    this.abort = null;
    this.generation += 1;
    this.root?.remove();
    this.root = null;
    this.list = null;
    this.items.clear();
  }

  /** Replaces the history for new bounds; keeps realtime additions that arrive meanwhile. */
  async load(bounds: TimeBounds): Promise<void> {
    this.abort?.abort();
    const controller = new AbortController();
    this.abort = controller;
    const generation = ++this.generation;
    this.pendingRealtime = [];
    this.setStatus('History: loading');
    let history: OpsAlarm[] = [];
    try {
      history = await this.deps.client.fetchAlarmHistory(bounds.start, bounds.end, controller.signal);
      if (generation !== this.generation) return;
      this.setStatus(`History: ${history.length} alarm messages in the time bounds`);
    } catch (error) {
      if (generation !== this.generation) return;
      // Only our own error text is shown; never response content.
      this.setStatus(
        error instanceof GroundLinkError || error instanceof RangeError
          ? `History unavailable: ${error.message}`
          : 'History unavailable: request failed',
      );
    }
    // Old bounds' alarms are removed even when the fetch failed, so nothing from a previous
    // time range is presented as belonging to the new one. Realtime messages that arrived while
    // loading are kept.
    const realtime = this.pendingRealtime ?? [];
    this.pendingRealtime = null;
    this.model.clear();
    for (const m of history) this.model.add(m);
    for (const m of realtime) this.model.add(m);
    this.render();
  }

  private setStatus(text: string): void {
    if (this.statusLine) this.statusLine.textContent = text;
  }

  private render(): void {
    const list = this.list;
    if (list === null) return;
    const doc = list.ownerDocument;
    const groups = this.model.groups();
    const keep = new Set(groups.map((g) => g.key));
    for (const [key, li] of this.items) {
      if (!keep.has(key)) {
        li.remove();
        this.items.delete(key);
      }
    }
    groups.forEach((group, index) => {
      let li = this.items.get(group.key);
      if (li === undefined) {
        li = doc.createElement('li');
        li.className = 'caelum-alarm';
        li.tabIndex = 0;
        li.dataset['alarmKey'] = group.key;
        this.items.set(group.key, li);
      }
      this.fillGroup(doc, li, group);
      if (list.children[index] !== li) list.insertBefore(li, list.children[index] ?? null);
    });
    if (groups.length === 0) {
      list.setAttribute('aria-label', 'No alarm messages');
    } else {
      list.removeAttribute('aria-label');
    }
  }

  private fillGroup(doc: Document, li: HTMLLIElement, group: AlarmGroup): void {
    const latest = group.transitions[group.transitions.length - 1];
    if (latest === undefined) return;
    const label = crewLabel(this.deps.crew, group.crewId);
    const head = doc.createElement('p');
    head.className = 'caelum-alarm__head';
    const title = doc.createElement('span');
    title.className = 'caelum-alarm__title';
    title.textContent =
      ` ${label} · ${CATEGORY_TEXT[group.category]} · created ${group.createdAt}` +
      ` · ${STATE_TEXT[latest.state]} · escalation stage ${latest.escalationStage}`;
    head.append(statusBadge(doc, latest.priority), title);

    const transitions = doc.createElement('ol');
    transitions.className = 'caelum-alarm__transitions';
    for (const t of group.transitions) {
      const item = doc.createElement('li');
      item.dataset['state'] = t.state;
      item.textContent = transitionText(t);
      transitions.append(item);
    }
    li.dataset['priority'] = latest.priority;
    li.setAttribute(
      'aria-label',
      `${label}, ${CATEGORY_TEXT[group.category]}, ${STATUS_PRESENTATION[latest.priority].text}, ` +
        `${STATE_TEXT[latest.state]}, created ${group.createdAt}, ` +
        `${group.transitions.length} transitions`,
    );
    li.replaceChildren(head, transitions);
  }
}

export function createAlertTimelineViewProvider(deps: AlertTimelineDeps): ViewProvider {
  return {
    key: 'caelum.alert-timeline-view',
    name: 'CAELUM Alert Timeline',
    cssClass: 'icon-timeline',
    canView: (domainObject: DomainObject) => domainObject.type === TYPE_ALERT_TIMELINE,
    view: () => new AlertTimelineView(deps),
  };
}
