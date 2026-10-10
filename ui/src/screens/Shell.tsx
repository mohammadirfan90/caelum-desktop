import { useEffect, useId, useState } from "react";

import type { AlarmView } from "../alarmStore";
import type { Api, DataEvents } from "../api";
import type { CrewMember, SessionInfo } from "../bindings";
import { useLoad } from "../components/useLoad";
import { EmptyState, NoData } from "../components/Values";
import { type Level, LevelGlyph } from "../components/LevelGlyph";
import { type Screen, screensFor } from "../navigation";
import { type Prefs, TEXT_SCALES, type Theme } from "../settings";
import { strings } from "../strings";
import { Administration } from "./Admin";
import { Alerts } from "./Alerts";
import { Audit } from "./Audit";
import { Checks } from "./Checks";
import { Body, type CrewContext, Home } from "./Crew";
import { Devices } from "./Devices";
import { Environment } from "./Environment";
import { Ground } from "./Ground";
import { Medical } from "./Medical";

/** Status footer (docs/07 §3): device links, ground link and instance role from the core; NO DATA
 * when the role may not read them or they are not known. */
function Footer({ api, version }: { readonly api: Api; readonly version: number }) {
  const devices = useLoad(() => api.deviceList(), `d${String(version)}`);
  const ground = useLoad(() => api.groundStatus(), `g${String(version)}`);
  return (
    <footer aria-label={strings.footerLabel} className="flex flex-wrap gap-6 border-t border-border px-5 py-2 text-sm">
      <span className="flex items-center gap-2">
        {strings.linkState}
        {devices.state === "ok" ? (
          <span>{strings.footerDevices(devices.data.filter((d) => d.status === "connected").length, devices.data.length)}</span>
        ) : (
          <NoData status="noData" />
        )}
      </span>
      <span className="flex items-center gap-2">
        {strings.groundLink}
        {ground.state === "ok" ? <span>{strings.footerPathB[String(ground.data.pathBConfigured)]}</span> : <NoData status="noData" />}
      </span>
      <span className="flex items-center gap-2">
        {strings.instanceRole}
        {ground.state === "ok" ? (
          <span>{strings.footerInstance[ground.data.instanceRole] ?? ground.data.instanceRole}</span>
        ) : (
          <NoData status="noData" />
        )}
      </span>
    </footer>
  );
}

const NO_EVENTS: DataEvents = {
  onObservations: () => Promise.resolve(() => undefined),
  onDevices: () => Promise.resolve(() => undefined),
};

/** Crew-wide emergency view (F-ALM-07, SR-014): every active alarm on this station. */
export function EmergencyView({
  alarms,
  audited,
  onBack,
}: {
  readonly alarms: readonly AlarmView[];
  readonly audited: boolean;
  readonly onBack: () => void;
}) {
  return (
    <section aria-labelledby="emergency-title" className="flex w-full max-w-[48rem] flex-col gap-4">
      <h2 id="emergency-title" className="text-2xl font-semibold">
        {strings.emergencyTitle}
      </h2>
      {audited ? null : (
        <p role="alert" className="text-sm font-semibold">
          {strings.emergencyNotAudited}
        </p>
      )}
      <h3 className="label">{strings.emergencyAlarms}</h3>
      {alarms.length === 0 ? (
        <EmptyState title={strings.emergencyNoAlarms} />
      ) : (
        <ul className="flex flex-col gap-2">
          {alarms.map((a) => (
            <li key={a.alarm} className="card flex items-center gap-3 p-4">
              <LevelGlyph level={asLevel(a.priority)} size={24} />
              <span className="font-semibold">{`${strings.level[asLevel(a.priority)]}: ${a.title}`}</span>
            </li>
          ))}
        </ul>
      )}
      <button type="button" className="btn btn-soft self-start" onClick={onBack}>
        {strings.back}
      </button>
    </section>
  );
}

function asLevel(p: string): Level {
  return p === "critical" || p === "warning" || p === "observe" ? p : "technical";
}

/** Display settings (F-SYS-10). */
export function SettingsScreen({ prefs, onChange }: { readonly prefs: Prefs; readonly onChange: (p: Prefs) => void }) {
  const themes: Theme[] = ["light", "dark", "high-contrast"];
  return (
    <section aria-labelledby="settings-title" className="flex max-w-[34rem] flex-col gap-6">
      <h2 id="settings-title" className="text-2xl font-semibold">
        {strings.settingsTitle}
      </h2>
      <fieldset className="flex flex-col gap-2">
        <legend className="label">{strings.theme}</legend>
        {themes.map((t) => (
          <label key={t} className="flex min-h-12 items-center gap-3">
            <input
              type="radio"
              name="theme"
              checked={prefs.theme === t}
              onChange={() => {
                onChange({ ...prefs, theme: t });
              }}
            />
            {strings.themes[t]}
          </label>
        ))}
      </fieldset>
      <fieldset className="flex flex-col gap-2">
        <legend className="label">{strings.textSize}</legend>
        {TEXT_SCALES.map((s) => (
          <label key={s} className="flex min-h-12 items-center gap-3">
            <input
              type="radio"
              name="text-size"
              checked={prefs.textScale === s}
              onChange={() => {
                onChange({ ...prefs, textScale: s });
              }}
            />
            {strings.textScale(s)}
          </label>
        ))}
      </fieldset>
      <label className="flex min-h-12 items-center gap-3">
        <input
          type="checkbox"
          checked={prefs.glove}
          onChange={(e) => {
            onChange({ ...prefs, glove: e.target.checked });
          }}
        />
        {strings.gloveMode}
      </label>
      <label className="flex min-h-12 items-center gap-3">
        <input
          type="checkbox"
          checked={prefs.reducedMotion}
          onChange={(e) => {
            onChange({ ...prefs, reducedMotion: e.target.checked });
          }}
        />
        {strings.reducedMotion}
      </label>
    </section>
  );
}

/**
 * The signed-in layout (docs/07 §3): nav rail by role (ROLE_MATRIX §6), crew strip, content and
 * status footer. Values not yet available on this station show NO DATA, never a placeholder.
 */
export function Shell({
  api,
  session,
  prefs,
  onPrefs,
  onSignOut,
  alarms = [],
  events = NO_EVENTS,
}: {
  readonly api: Api;
  readonly session: SessionInfo;
  readonly prefs: Prefs;
  readonly onPrefs: (p: Prefs) => void;
  readonly onSignOut: () => void;
  /** Open alarms from the alarm bus (no values). */
  readonly alarms?: readonly AlarmView[];
  /** Value-free change events (new readings, device changes). */
  readonly events?: DataEvents;
}) {
  const screens = screensFor(session.role);
  const [screen, setScreen] = useState<Screen>(screens[0] ?? "settings");
  const crewSelectId = useId();
  const [crew, setCrew] = useState<CrewMember[]>([]);
  const [chosen, setChosen] = useState<string | null>(null);
  const [observed, setObserved] = useState(0);
  const [deviceVersion, setDeviceVersion] = useState(0);
  const seesCrew = session.role !== "mcc" && session.role !== "device_gateway";
  useEffect(() => {
    if (!seesCrew) return;
    let current = true;
    void api.crewList().then((r) => {
      if (current && r.status === "ok") setCrew(r.data);
    });
    return () => {
      current = false;
    };
  }, [api, seesCrew]);
  // An astronaut sees only their own data (ROLE_MATRIX §2); others choose a crew member.
  const crewId = session.role === "astronaut" ? session.crewMemberId : chosen;
  useEffect(() => {
    let stops: (() => void)[] = [];
    let current = true;
    void Promise.all([
      events.onObservations((id) => {
        if (id === crewId) setObserved((n) => n + 1);
      }),
      events.onDevices(() => {
        setDeviceVersion((n) => n + 1);
      }),
    ]).then((s) => {
      if (current) {
        stops = s;
      } else {
        s.forEach((stop) => {
          stop();
        });
      }
    });
    return () => {
      current = false;
      stops.forEach((stop) => {
        stop();
      });
    };
  }, [events, crewId]);
  const ctx: CrewContext = { api, crewId, version: observed, alarms };
  const alarmVersion = alarms.length + alarms.reduce((n, a) => n + a.stages.length, 0);
  const crewName = crew.find((c) => c.id === crewId)?.name;
  return (
    <div className="flex min-h-0 flex-1">
      <nav aria-label={strings.navLabel} className="w-48 shrink-0 border-r border-border bg-surface p-3">
        <ul className="flex flex-col gap-1">
          {screens.map((s) => (
            <li key={s}>
              <button
                type="button"
                aria-current={screen === s ? "page" : undefined}
                className={`btn w-full justify-start ${screen === s ? "btn-ink" : "btn-soft"}`}
                onClick={() => {
                  setScreen(s);
                }}
              >
                {strings.screen[s]}
              </button>
            </li>
          ))}
        </ul>
      </nav>
      <div className="flex min-w-0 flex-1 flex-col">
        <section aria-label={strings.crewStripLabel} className="flex items-center justify-between gap-4 border-b border-border px-5 py-3">
          {session.role === "astronaut" || !seesCrew ? (
            <span className="text-sm text-ink-3">{crewName ?? strings.crewNotSelected}</span>
          ) : (
            <span className="flex items-center gap-2">
              <label htmlFor={crewSelectId} className="text-sm font-semibold">
                {strings.crewSelect}
              </label>
              <select
                id={crewSelectId}
                className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
                value={chosen ?? ""}
                onChange={(e) => {
                  setChosen(e.target.value === "" ? null : e.target.value);
                }}
              >
                <option value="">{crew.length === 0 ? strings.crewNoRoster : strings.crewSelectNone}</option>
                {crew
                  .filter((c) => c.active)
                  .map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
              </select>
            </span>
          )}
          <span className="flex items-center gap-3">
            <span className="text-sm font-semibold">{strings.signedInAs(session.username, strings.role[session.role])}</span>
            <button type="button" className="btn btn-soft" onClick={onSignOut}>
              {strings.signOut}
            </button>
          </span>
        </section>
        {/* Focusable so keyboard users can scroll it (axe scrollable-region-focusable). */}
        {/* eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex */}
        <main id="content" tabIndex={0} className="flex-1 overflow-auto p-6">
          {screen === "settings" ? (
            <div className="flex flex-col gap-10">
              <SettingsScreen prefs={prefs} onChange={onPrefs} />
              {session.role === "administrator" ? <Administration api={api} self={session.userId} /> : null}
            </div>
          ) : (
            <section aria-labelledby="screen-title" className="flex flex-col gap-4">
              <h2 id="screen-title" className="text-2xl font-semibold">
                {strings.screen[screen]}
              </h2>
              {screen === "home" ? <Home ctx={ctx} /> : null}
              {screen === "body" ? <Body ctx={ctx} /> : null}
              {screen === "alerts" ? <Alerts api={api} userRole={session.role} crew={crew} version={alarmVersion} /> : null}
              {screen === "devices" ? <Devices api={api} userRole={session.role} crew={crew} version={deviceVersion} /> : null}
              {screen === "audit" ? <Audit api={api} version={deviceVersion} /> : null}
              {screen === "checks" ? <Checks api={api} crewId={crewId} isAstronaut={session.role === "astronaut"} /> : null}
              {screen === "medical" ? (
                <Medical api={api} userRole={session.role} self={session.userId} crewId={crewId} crew={crew} version={observed} />
              ) : null}
              {screen === "env" ? <Environment api={api} crewId={crewId} version={observed} /> : null}
              {screen === "ground" ? <Ground api={api} version={deviceVersion} /> : null}
            </section>
          )}
        </main>
        <Footer api={api} version={deviceVersion} />
      </div>
    </div>
  );
}
