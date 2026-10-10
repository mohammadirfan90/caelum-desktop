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
import { RulesetBanner } from "../../src/RulesetBanner";
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
        render(<main className="bg-canvas p-4 text-ink">{make()}</main>);
        await settle();
        expect(await violations()).toEqual([]);
        if (variant === "200-glove") await page.screenshot({ path: `__screenshots__/${name}-200-glove.png` });
      });
    }
    it(`signed-in shell in ${variant}`, async () => {
      applyPrefs(prefs);
      render(<Shell session={session} prefs={prefs} onPrefs={() => undefined} onSignOut={() => undefined} />);
      await settle();
      expect(await violations()).toEqual([]);
      if (variant === "200-glove") await page.screenshot({ path: "__screenshots__/shell-200-glove.png" });
    });
  }

  const shell = () => <Shell session={session} prefs={DEFAULT_PREFS} onPrefs={() => undefined} onSignOut={() => undefined} />;
  for (const [name, make] of [...Object.entries(SCREENS), ["shell", shell] as const]) {
    it(`${name}: text doubles at 200 % and nothing is clipped (docs/07 §5)`, async () => {
      applyPrefs(DEFAULT_PREFS);
      const view = render(<main className="bg-canvas p-4 text-ink">{make()}</main>);
      await settle();
      const normal = textSizes();
      applyPrefs({ ...DEFAULT_PREFS, textScale: 200, glove: true });
      view.rerender(<main className="bg-canvas p-4 text-ink">{make()}</main>);
      await settle();
      const large = textSizes();
      expect(normal.length).toBeGreaterThan(0);
      expect(large).toEqual(normal.map((n) => n * 2));
      expect(clipped()).toEqual([]);
    });
  }
});
