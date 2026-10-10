// M5 gate (F-SYS-10, docs/07 §1–3): axe accessibility checks (WCAG 2.1 A and AA, incl. colour
// contrast as rendered) on every screen and primitive, in the light, dark and high-contrast themes
// and at 200 % text in glove mode; at 200 % every piece of text is exactly twice its 100 % size and
// none is clipped; screenshots of the 200 % / glove variants for review.
import "../../src/styles/index.css";

import { cleanup, render } from "@testing-library/react";
import axe from "axe-core";
import type { ReactElement } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { page } from "vitest/browser";

import { type AlarmMap, ranked, reduce } from "../../src/alarmStore";
import type { AlarmBusEvent, SessionInfo } from "../../src/bindings";
import { AlarmBar } from "../../src/components/AlarmBar";
import { LevelBadge } from "../../src/components/LevelGlyph";
import { Card, EmptyState, NoData, SectionHead, Stat } from "../../src/components/Values";
import type { Api } from "../../src/api";
import { RulesetBanner } from "../../src/RulesetBanner";
import { Administration } from "../../src/screens/Admin";
import { Alerts } from "../../src/screens/Alerts";
import { Audit } from "../../src/screens/Audit";
import { Body, Home } from "../../src/screens/Crew";
import { Devices } from "../../src/screens/Devices";
import { Locked, SignIn } from "../../src/screens/Access";
import { EmergencyView, Shell } from "../../src/screens/Shell";
import { applyPrefs, DEFAULT_PREFS, type Prefs } from "../../src/settings";

const ok = () => Promise.resolve({ status: "ok" as const });

const events: AlarmBusEvent[] = [
  { type: "alarm", alarm: "c", crewId: "c1", priority: "critical", category: "RESPIRATORY", state: "active", title: "Severe low blood oxygen" },
  { type: "alarm", alarm: "w", crewId: "c2", priority: "warning", category: "CARDIO", state: "confirming", title: "High heart rate at rest" },
  { type: "stage", alarm: "c", stage: 0, audience: "astronaut" },
  { type: "stage", alarm: "c", stage: 2, audience: "sound" },
];
const alarms = ranked(events.reduce<AlarmMap>(reduce, new Map()));

const session: SessionInfo = { userId: "u1", username: "cmo.one", role: "cmo", crewMemberId: "c1", locked: false };

/** Station answers for the screens that read data (fixtures, shown as such). */
const api = {
  crewList: () =>
    Promise.resolve({
      status: "ok" as const,
      data: [
        { id: "c1", name: "Fixture Astronaut A", position: "flight engineer", mission: "TEST-1", active: true },
        { id: "c2", name: "Fixture Officer B", position: "crew medical officer", mission: "TEST-1", active: false },
      ],
    }),
  userList: () =>
    Promise.resolve({
      status: "ok" as const,
      data: [
        { id: "u0", username: "admin", role: "administrator" as const, crewMemberId: null, disabled: false },
        { id: "u1", username: "astro.a", role: "astronaut" as const, crewMemberId: "c1", disabled: false },
        { id: "u2", username: "cmo.b", role: "cmo" as const, crewMemberId: "c2", disabled: true },
      ],
    }),
  vitalsLatest: () =>
    Promise.resolve({
      status: "ok" as const,
      data: [
        { reading: { parameter: "spo2", value: 97, text: null, unit: "%", measuredAtTai: "1", measuredAtUtcMs: Date.UTC(2026, 9, 10, 12), quality: "good", classification: "measured", deviceId: "d1" }, status: "live" as const, ageS: 2, deviceModel: "Oximeter X" },
        { reading: { parameter: "heart_rate", value: 72, text: null, unit: "/min", measuredAtTai: "1", measuredAtUtcMs: Date.UTC(2026, 9, 10, 11), quality: "good", classification: "measured", deviceId: "d1" }, status: "stale" as const, ageS: 3600, deviceModel: "Strap Y" },
      ],
    }),
  vitalsTrend: () =>
    Promise.resolve({
      status: "ok" as const,
      data: {
        points: [70, 72, 75, 73].map((v, i) => ({ parameter: "heart_rate", value: v, text: null, unit: "/min", measuredAtTai: String(i), measuredAtUtcMs: Date.UTC(2026, 9, 10, 12, i), quality: "good", classification: "measured", deviceId: "d1" })),
        baseline: { median: 71, mad: 2, n: 500, status: "established" },
      },
    }),
  waveformRecent: () => Promise.resolve({ status: "ok" as const, data: [] }),
  alarmList: () =>
    Promise.resolve({
      status: "ok" as const,
      data: [
        { id: "a1", crewMemberId: "c1", kind: "physiological", category: "RESPIRATORY", priority: "critical", state: "active", title: "Severe low blood oxygen", ruleId: "spo2.critical", createdAtTai: "1", createdAtUtcMs: Date.UTC(2026, 9, 10, 12), acknowledgedAtUtcMs: null, resolvedAtUtcMs: null, acknowledgedAtTai: null, groundAcknowledgedAtTai: null, silencedUntilTai: null, resolvedAtTai: null, escalationStage: 2, opsOnly: false },
      ],
    }),
  alarmReport: () =>
    Promise.resolve({
      status: "ok" as const,
      data: { fromTai: "0", toTai: "1", history: [], rules: [{ ruleId: "spo2.critical", priority: "critical", count: 2, medianAckS: 31, ackedEarly: 1, silenced: 0, selfResolved: 0, repeatBursts: 0 }], perCrew: [] },
    }),
  deviceList: () =>
    Promise.resolve({
      status: "ok" as const,
      data: [
        { id: "d1", adapter: "ble-sig", model: "Oximeter X", vendor: "unknown", serial: "AA:BB:CC:DD:EE:FF", firmware: null, crewMemberId: "c1", deviceUser: null, status: "connected", lastSeenTai: "1", lastSeenUtcMs: Date.UTC(2026, 9, 10, 12), clockOffsetMs: 3.2, batteryPct: 81 },
        { id: "d2", adapter: "ble-wristband", model: "Band Z", vendor: "unknown", serial: "11:22:33:44:55:66", firmware: null, crewMemberId: null, deviceUser: null, status: "registered", lastSeenTai: null, lastSeenUtcMs: null, clockOffsetMs: null, batteryPct: null },
      ],
    }),
  auditVerify: () => Promise.resolve({ status: "ok" as const, data: { entries: "3", intact: false, problems: [{ seq: "3", kind: "hashMismatch" as const }] } }),
  auditList: () =>
    Promise.resolve({
      status: "ok" as const,
      data: [{ seq: "1", id: "e1", atTai: "1", atUtcMs: Date.UTC(2026, 9, 10, 12), actor: "system", role: "system", action: "key.create", target: null, details: "{}" }],
    }),
} as unknown as Api;

const crew = [{ id: "c1", name: "Fixture Astronaut A", position: "flight engineer", mission: "TEST-1", active: true }];

function Gallery() {
  return (
    <div className="flex flex-col gap-4 p-4">
      <Card label="Levels">
        <SectionHead title="Levels" />
        <div className="flex flex-wrap gap-4">
          {(["normal", "observe", "warning", "critical", "technical"] as const).map((l) => (
            <LevelBadge key={l} level={l} />
          ))}
        </div>
      </Card>
      <Card label="Values">
        <div className="flex flex-col gap-4">
        <Stat label="SpO2" reading={{ value: 97, unit: "%", at: "12:00:01 UTC", source: "Oximeter 3" }} />
        <Stat label="Heart rate" reading={null} missing="stale" />
        <NoData status="unusable" />
        <EmptyState detail="Nothing recorded." />
        </div>
      </Card>
    </div>
  );
}

const SCREENS: Record<string, () => ReactElement> = {
  "sign-in": () => <SignIn needsSetup={false} onSignIn={ok} onCreateAdministrator={ok} onEmergency={() => undefined} />,
  "first-administrator": () => <SignIn needsSetup onSignIn={ok} onCreateAdministrator={ok} onEmergency={() => undefined} />,
  locked: () => <Locked name="astro.a" onUnlock={ok} onSignOut={() => undefined} onEmergency={() => undefined} />,
  emergency: () => <EmergencyView alarms={alarms} audited={false} onBack={() => undefined} />,
  "alarm-bar": () => <AlarmBar alarms={alarms} onAcknowledge={() => undefined} onSilence={() => undefined} />,
  "ruleset-banner": () => (
    <RulesetBanner load={() => Promise.resolve({ status: "ok", data: { state: "fallback", version: "1.0.0-draft.2", development: true, reason: "digest mismatch" } })} />
  ),
  gallery: () => <Gallery />,
  administration: () => <Administration api={api} self="u0" />,
  home: () => <Home ctx={{ api, crewId: "c1", version: 0, alarms }} />,
  body: () => <Body ctx={{ api, crewId: "c1", version: 0, alarms }} />,
  alerts: () => <Alerts api={api} userRole="cmo" crew={crew} version={0} />,
  devices: () => <Devices api={api} userRole="cmo" crew={crew} version={0} />,
  audit: () => <Audit api={api} version={0} />,
};

const VARIANTS: Record<string, Prefs> = {
  light: DEFAULT_PREFS,
  dark: { ...DEFAULT_PREFS, theme: "dark" },
  "high-contrast": { ...DEFAULT_PREFS, theme: "high-contrast" },
  "200-glove": { ...DEFAULT_PREFS, textScale: 200, glove: true },
};

async function violations(): Promise<string[]> {
  const result = await axe.run(document.body, {
    runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"] },
  });
  return result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
}

/** Computed font size of every element that directly holds text, in document order. */
function textSizes(): number[] {
  return [...document.body.querySelectorAll<HTMLElement>("*")]
    .filter((e) => [...e.childNodes].some((n) => n.nodeType === Node.TEXT_NODE && (n.textContent ?? "").trim() !== ""))
    .filter((e) => !e.closest(".sr-only"))
    .map((e) => parseFloat(getComputedStyle(e).fontSize));
}

/** Elements whose text is cut off (overflow hidden or ellipsis with content wider than the box). */
function clipped(): string[] {
  return [...document.body.querySelectorAll<HTMLElement>("*")]
    .filter((e) => !e.closest(".sr-only"))
    .filter((e) => {
      const st = getComputedStyle(e);
      const hides = st.overflowX === "hidden" || st.overflowX === "clip" || st.textOverflow === "ellipsis";
      return hides && e.scrollWidth > e.clientWidth + 1;
    })
    .map((e) => `${e.tagName.toLowerCase()} "${e.textContent.slice(0, 40)}"`);
}

const settle = () => new Promise((r) => setTimeout(r, 50));

afterEach(() => {
  cleanup();
});

describe("accessibility (axe, WCAG 2.1 AA)", () => {
  it("detects violations (self-test: unlabelled input, low-contrast text)", async () => {
    applyPrefs(DEFAULT_PREFS);
    render(
      <main>
        <input />
        <p style={{ color: "#bbb", background: "#fff" }}>faint</p>
      </main>,
    );
    const found = await violations();
    expect(found.some((v) => v.startsWith("label:"))).toBe(true);
    expect(found.some((v) => v.startsWith("color-contrast:"))).toBe(true);
  });

  for (const [variant, prefs] of Object.entries(VARIANTS)) {
    for (const [name, make] of Object.entries(SCREENS)) {
      it(`${name} in ${variant}`, async () => {
        applyPrefs(prefs);
        document.body.style.background = "var(--canvas)";
        render(
          // The scroll region is focusable, as in the shell (scrollable-region-focusable).
          // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex
          <main tabIndex={0} className="h-screen overflow-auto bg-canvas p-4 text-ink">
            {make()}
          </main>,
        );
        await settle();
        expect(await violations()).toEqual([]);
        if (variant === "200-glove") await page.screenshot({ path: `__screenshots__/${name}-200-glove.png` });
      });
    }
    it(`signed-in shell in ${variant}`, async () => {
      applyPrefs(prefs);
      render(<Shell api={api} session={session} prefs={prefs} onPrefs={() => undefined} onSignOut={() => undefined} />);
      await settle();
      expect(await violations()).toEqual([]);
      if (variant === "200-glove") await page.screenshot({ path: "__screenshots__/shell-200-glove.png" });
    });
  }

  const shell = () => <Shell api={api} session={session} prefs={DEFAULT_PREFS} onPrefs={() => undefined} onSignOut={() => undefined} />;
  for (const [name, make] of [...Object.entries(SCREENS), ["shell", shell] as const]) {
    it(`${name}: text doubles at 200 % and nothing is clipped (docs/07 §5)`, async () => {
      applyPrefs(DEFAULT_PREFS);
      const view = render(
          // The scroll region is focusable, as in the shell (scrollable-region-focusable).
          // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex
          <main tabIndex={0} className="h-screen overflow-auto bg-canvas p-4 text-ink">
            {make()}
          </main>,
        );
      await settle();
      const normal = textSizes();
      applyPrefs({ ...DEFAULT_PREFS, textScale: 200, glove: true });
      view.rerender(
          // The scroll region is focusable, as in the shell (scrollable-region-focusable).
          // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex
          <main tabIndex={0} className="h-screen overflow-auto bg-canvas p-4 text-ink">
            {make()}
          </main>,
        );
      await settle();
      const large = textSizes();
      expect(normal.length).toBeGreaterThan(0);
      expect(large).toEqual(normal.map((n) => n * 2));
      expect(clipped()).toEqual([]);
    });
  }
});
