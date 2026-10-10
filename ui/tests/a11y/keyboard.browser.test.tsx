// M5 gate (F-SYS-10, docs/07 §5): keyboard-only walkthrough of the whole window in a real browser.
// Real key presses only (Tab, Shift+Tab, Enter, Space, arrows; no clicks): skip link, sign-in,
// role navigation, display settings, acknowledging an alarm from the alarm bar, and the emergency
// view without sign-in (SR-014). Every focused control shows a visible focus outline. The whole
// window passes axe at each step.
import "../../src/styles/index.css";

import { cleanup, render, screen } from "@testing-library/react";
import axe from "axe-core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { userEvent } from "vitest/browser";

import { type Api, App } from "../../src/App";
import type { AlarmBusEvent, CommandError, SessionInfo } from "../../src/bindings";
import { applyPrefs, DEFAULT_PREFS } from "../../src/settings";

const astronaut: SessionInfo = { userId: "u1", username: "astro.a", role: "astronaut", crewMemberId: "c1", locked: false };
const signInRequired = { status: "error" as const, error: { kind: "signInRequired", message: "", retryAfterS: null } satisfies CommandError };

function station() {
  let signedIn = false;
  let handler: ((e: AlarmBusEvent) => void) | undefined;
  const api: Partial<Api> = {
    appInfo: () => Promise.resolve({ productName: "CAELUM", version: "0.1.0", kiosk: false }),
    rulesetStatus: () => Promise.resolve({ status: "ok", data: { state: "verified", version: "1.0.0", development: false, reason: null } }),
    setupStatus: () => Promise.resolve({ status: "ok", data: false }),
    authMe: () => Promise.resolve(signedIn ? { status: "ok", data: astronaut } : signInRequired),
    authLogin: vi.fn((username: string, secret: string) => {
      signedIn = username === "astro.a" && secret === "246813";
      return Promise.resolve(signedIn ? { status: "ok" as const, data: astronaut } : signInRequired);
    }),
    authLogout: () => {
      signedIn = false;
      return Promise.resolve({ status: "ok", data: null });
    },
    alarmAcknowledge: vi.fn(() => Promise.resolve({ status: "ok" as const, data: "onboard" as const })),
    breakGlassOpen: () => Promise.resolve({ status: "ok", data: { opened: true, audited: true } }),
    crewList: () => Promise.resolve({ status: "ok", data: [{ id: "c1", name: "Fixture Astronaut A", position: "flight engineer", mission: "TEST-1", active: true }] }),
    vitalsLatest: () => Promise.resolve({ status: "ok", data: [] }),
  };
  const subscribe = (h: (e: AlarmBusEvent) => void) => {
    handler = h;
    return Promise.resolve(() => undefined);
  };
  render(
    <App
      api={api}
      subscribeAlarms={subscribe}
      pollMs={3_600_000}
      dataEvents={{ onObservations: () => Promise.resolve(() => undefined), onDevices: () => Promise.resolve(() => undefined) }}
      makeAudio={() => ({ loop: false, currentTime: 0, play: () => Promise.resolve(), pause: () => undefined })}
    />,
  );
  return { api, emit: (e: AlarmBusEvent) => handler?.(e) };
}

/** The focused element; fails if focus is on nothing or its outline is not visible. */
function focused(): HTMLElement {
  const el = document.activeElement;
  if (!(el instanceof HTMLElement) || el === document.body) throw new Error("nothing has focus");
  if (el.tabIndex >= 0) {
    const st = getComputedStyle(el);
    expect(st.outlineStyle, `focus outline on ${el.outerHTML.slice(0, 60)}`).not.toBe("none");
    expect(parseFloat(st.outlineWidth)).toBeGreaterThanOrEqual(2);
  }
  return el;
}

/** Accessible name, enough for these controls: aria-label, the input's label, or its text. */
function name(el: HTMLElement): string {
  const label = el instanceof HTMLInputElement ? el.labels?.[0]?.textContent : undefined;
  return (el.getAttribute("aria-label") ?? label ?? el.textContent).trim();
}

/** Presses Tab until the focused control's name matches; fails after `max` presses. */
async function tabTo(match: RegExp, max = 40): Promise<HTMLElement> {
  for (let i = 0; i < max; i++) {
    await userEvent.keyboard("{Tab}");
    // Past the last control focus leaves the page (browser chrome) and the next Tab wraps around.
    if (document.activeElement === document.body) continue;
    const el = focused();
    if (match.test(name(el))) return el;
  }
  throw new Error(`no control matching ${String(match)} within ${String(max)} Tab presses`);
}

/** Checks the settled page: CSS transitions (e.g. after a theme change) finish first. */
async function noViolations() {
  await Promise.all(document.getAnimations().map((a) => a.finished));
  const result = await axe.run(document.body, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"] } });
  expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
}

afterEach(() => {
  cleanup();
  applyPrefs(DEFAULT_PREFS);
});

describe("keyboard-only walkthrough", () => {
  it("skip link, sign-in, navigation, settings and alarm acknowledgement without a pointer", async () => {
    applyPrefs(DEFAULT_PREFS);
    const { api, emit } = station();
    await screen.findByRole("form", { name: "Sign in" });
    await noViolations();

    // The first Tab reaches the skip link; Enter moves focus to the content.
    await userEvent.keyboard("{Tab}");
    expect(name(focused())).toBe("Skip to content");
    await userEvent.keyboard("{Enter}");
    expect(document.activeElement?.id).toBe("content");

    // Sign in with the keyboard only.
    await userEvent.keyboard("{Tab}");
    expect(focused()).toBe(screen.getByLabelText("Sign-in name"));
    await userEvent.keyboard("astro.a{Tab}246813{Enter}");
    const nav = await screen.findByRole("navigation", { name: "Screens" });
    expect(api.authLogin).toHaveBeenCalledWith("astro.a", "246813");
    // The submitted form is gone: focus moves to the new content, not to the start of the page.
    expect(document.activeElement?.id).toBe("content");
    await noViolations();

    // Role navigation: reach Settings by Tab, open it with Enter.
    const settings = await tabTo(/^Settings$/);
    expect(nav.contains(settings)).toBe(true);
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByRole("heading", { name: "Display" })).toBeInTheDocument();
    expect(settings).toHaveAttribute("aria-current", "page");

    // Theme by arrow keys inside the radio group, glove mode by Space.
    await tabTo(/^Light$/);
    await userEvent.keyboard("{ArrowDown}");
    expect(document.documentElement.dataset.theme).toBe("dark");
    await tabTo(/^Glove/);
    await userEvent.keyboard(" ");
    expect(document.documentElement.dataset.glove).toBe("on");
    await noViolations();

    // An alarm arrives: Shift+Tab back to the alarm bar and acknowledge with Enter.
    emit({ type: "alarm", alarm: "a1", crewId: "c1", priority: "warning", category: "CARDIO", state: "active", title: "High heart rate at rest" });
    await screen.findByRole("button", { name: "Acknowledge" });
    let ack: HTMLElement | null = null;
    for (let i = 0; i < 40 && ack === null; i++) {
      await userEvent.keyboard("{Shift>}{Tab}{/Shift}");
      if (name(focused()) === "Acknowledge") ack = focused();
    }
    expect(ack, "Acknowledge reachable with Shift+Tab").not.toBeNull();
    await userEvent.keyboard("{Enter}");
    expect(api.alarmAcknowledge).toHaveBeenCalledWith("a1");
  });

  it("opens the emergency view and returns without a pointer or sign-in (SR-014)", async () => {
    applyPrefs(DEFAULT_PREFS);
    station();
    await screen.findByRole("form", { name: "Sign in" });
    await tabTo(/^Emergency/);
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByRole("heading", { name: "Emergency view" })).toBeInTheDocument();
    expect(document.activeElement?.id).toBe("content");
    await noViolations();
    await tabTo(/^Back$/);
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByRole("form", { name: "Sign in" })).toBeInTheDocument();
    expect(document.activeElement?.id).toBe("content");
  });
});
