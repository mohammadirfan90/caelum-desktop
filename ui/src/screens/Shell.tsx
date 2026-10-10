import { useState } from "react";

import type { AlarmView } from "../alarmStore";
import type { SessionInfo } from "../bindings";
import { EmptyState, NoData } from "../components/Values";
import { type Level, LevelGlyph } from "../components/LevelGlyph";
import { type Screen, screensFor } from "../navigation";
import { type Prefs, TEXT_SCALES, type Theme } from "../settings";
import { strings } from "../strings";

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
  session,
  prefs,
  onPrefs,
  onSignOut,
}: {
  readonly session: SessionInfo;
  readonly prefs: Prefs;
  readonly onPrefs: (p: Prefs) => void;
  readonly onSignOut: () => void;
}) {
  const screens = screensFor(session.role);
  const [screen, setScreen] = useState<Screen>(screens[0] ?? "settings");
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
          <span className="text-sm text-ink-3">{strings.crewNotSelected}</span>
          <span className="flex items-center gap-3">
            <span className="text-sm font-semibold">{strings.signedInAs(session.username, strings.role[session.role])}</span>
            <button type="button" className="btn btn-soft" onClick={onSignOut}>
              {strings.signOut}
            </button>
          </span>
        </section>
        <main id="content" tabIndex={-1} className="flex-1 overflow-auto p-6">
          {screen === "settings" ? (
            <SettingsScreen prefs={prefs} onChange={onPrefs} />
          ) : (
            <section aria-labelledby="screen-title" className="flex flex-col gap-2">
              <h2 id="screen-title" className="text-2xl font-semibold">
                {strings.screen[screen]}
              </h2>
              <EmptyState detail={strings.screenNoData} />
            </section>
          )}
        </main>
        <footer aria-label={strings.footerLabel} className="flex flex-wrap gap-6 border-t border-border px-5 py-2 text-sm">
          <span className="flex items-center gap-2">
            {strings.linkState}
            <NoData status="noData" />
          </span>
          <span className="flex items-center gap-2">
            {strings.groundLink}
            <NoData status="noData" />
          </span>
          <span className="flex items-center gap-2">
            {strings.instanceRole}
            <NoData status="noData" />
          </span>
        </footer>
      </div>
    </div>
  );
}
