// The CAELUM window (F-SYS-06, F-SYS-01, SR-014, SR-015; docs/07 §3): identity from the core and
// never a made-up value (CLAUDE.md rule 2, R-CLN-02); first-administrator enrolment, sign-in,
// lock, break-glass and the signed-in shell; alarms visible and audible in every state.
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { type Api, App } from "../src/App";
import type { AudioLike } from "../src/alarmSound";
import type { AlarmBusEvent, AppInfo, CommandError, SessionInfo } from "../src/bindings";

const info: AppInfo = { productName: "CAELUM", version: "0.1.0" };

const err = (kind: CommandError["kind"], retryAfterS: number | null = null) => ({
  status: "error" as const,
  error: { kind, message: kind, retryAfterS },
});

const astronaut: SessionInfo = { userId: "u1", username: "astro.a", role: "astronaut", crewMemberId: "c1", locked: false };

/** A station with an account, nobody signed in, and a verified flight ruleset (no banner). */
function api(overrides: Partial<Api> = {}): Partial<Api> {
  return {
    appInfo: () => Promise.resolve(info),
    rulesetStatus: () =>
      Promise.resolve({ status: "ok" as const, data: { state: "verified" as const, version: "1.0.0", development: false, reason: null } }),
    setupStatus: () => Promise.resolve({ status: "ok" as const, data: false }),
    authMe: () => Promise.resolve(err("signInRequired")),
    crewList: () => Promise.resolve({ status: "ok" as const, data: [] }),
    vitalsLatest: () => Promise.resolve({ status: "ok" as const, data: [] }),
    ...overrides,
  };
}

const noAlarms = () => Promise.resolve(() => undefined);
const noData = { onObservations: () => Promise.resolve(() => undefined), onDevices: () => Promise.resolve(() => undefined) };

function renderApp(a: Partial<Api>, subscribe: Parameters<typeof App>[0]["subscribeAlarms"] = noAlarms) {
  return render(<App api={a} subscribeAlarms={subscribe} dataEvents={noData} makeAudio={() => ({ loop: false, currentTime: 0, play: () => Promise.resolve(), pause: () => undefined })} />);
}

describe("App", () => {
  it("shows a loading status until the core answers, with no version", () => {
    renderApp(api({ appInfo: () => new Promise<AppInfo>(() => undefined) }));
    expect(screen.getByRole("status")).toHaveTextContent("Starting…");
    expect(screen.queryByText(/Version \d/)).not.toBeInTheDocument();
  });

  it("shows the product name and the version reported over IPC", async () => {
    renderApp(api());
    expect(screen.getByRole("heading", { level: 1, name: "CAELUM" })).toBeInTheDocument();
    expect(await screen.findByText("Version 0.1.0")).toBeInTheDocument();
  });

  it("shows the failure, and no version, when the IPC call fails", async () => {
    renderApp(api({ appInfo: () => Promise.reject(new Error("IPC unavailable")) }));
    expect(await screen.findByText("Version unavailable")).toBeInTheDocument();
    expect(screen.getByText(/IPC unavailable/)).toBeInTheDocument();
    expect(screen.queryByText(/\d+\.\d+\.\d+/)).not.toBeInTheDocument();
  });

  it("reports non-Error rejections as text instead of crashing", async () => {
    // Tauri's invoke() rejects with plain strings (e.g. permission denials), so this case is real.
    // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
    renderApp(api({ appInfo: () => Promise.reject("command app_info not allowed") }));
    expect(await screen.findByText(/command app_info not allowed/)).toBeInTheDocument();
  });

  it("uses one main landmark, a skip link and the always-visible alarm bar", async () => {
    renderApp(api());
    await screen.findByRole("form", { name: "Sign in" });
    expect(screen.getAllByRole("main")).toHaveLength(1);
    expect(screen.getByRole("link", { name: "Skip to content" })).toHaveAttribute("href", "#content");
    expect(screen.getByRole("region", { name: "Active alarms" })).toHaveTextContent("No active alarms");
  });

  it("offers first-administrator enrolment on a new station (CR-007 §4)", async () => {
    let created = false;
    const create = vi.fn(() => {
      created = true;
      return Promise.resolve({ status: "ok" as const, data: null });
    });
    renderApp(api({ setupStatus: () => Promise.resolve({ status: "ok" as const, data: !created }), setupCreateAdministrator: create }));
    const form = await screen.findByRole("form", { name: "Create the administrator account" });
    fireEvent.change(screen.getByLabelText("Sign-in name"), { target: { value: "admin" } });
    fireEvent.change(screen.getByLabelText("PIN or password"), { target: { value: "a long enough passphrase" } });
    fireEvent.submit(form);
    await screen.findByRole("form", { name: "Sign in" });
    expect(create).toHaveBeenCalledWith("admin", "a long enough passphrase");
  });

  it("signs in and shows the shell with the role's screens only (ROLE_MATRIX §6)", async () => {
    let signedIn = false;
    renderApp(
      api({
        authLogin: () => {
          signedIn = true;
          return Promise.resolve({ status: "ok" as const, data: astronaut });
        },
        authMe: () => Promise.resolve(signedIn ? { status: "ok" as const, data: astronaut } : err("signInRequired")),
      }),
    );
    const form = await screen.findByRole("form", { name: "Sign in" });
    fireEvent.change(screen.getByLabelText("Sign-in name"), { target: { value: "astro.a" } });
    fireEvent.change(screen.getByLabelText("PIN or password"), { target: { value: "246813" } });
    fireEvent.submit(form);
    const nav = await screen.findByRole("navigation", { name: "Screens" });
    expect(nav).toHaveTextContent("Home");
    expect(nav).not.toHaveTextContent("Medical");
    expect(nav).not.toHaveTextContent("Ground");
    expect(screen.getByText("astro.a (Astronaut)")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("heading", { name: "Display" })).toBeInTheDocument();
    // The footer shows NO DATA for links not yet available, never a made-up state.
    expect(screen.getByRole("contentinfo", { name: "Station status" })).toHaveTextContent("NO DATA");
  });

  it("shows sign-in errors from the catalog, including throttling", async () => {
    renderApp(api({ authLogin: () => Promise.resolve(err("throttled", 30)) }));
    const form = await screen.findByRole("form", { name: "Sign in" });
    fireEvent.submit(form);
    expect(await screen.findByRole("alert")).toHaveTextContent("Too many attempts. Try again in 30 s.");
  });

  it("shows the lock screen and unlocks with re-authentication", async () => {
    let locked = true;
    const reauth = vi.fn(() => {
      locked = false;
      return Promise.resolve({ status: "ok" as const, data: null });
    });
    renderApp(api({ authMe: () => Promise.resolve({ status: "ok" as const, data: { ...astronaut, locked } }), authReauth: reauth }));
    const form = await screen.findByRole("form", { name: "Session locked" });
    fireEvent.change(screen.getByLabelText("PIN or password"), { target: { value: "246813" } });
    fireEvent.submit(form);
    await screen.findByRole("navigation", { name: "Screens" });
    expect(reauth).toHaveBeenCalledWith("246813");
  });

  it("opens the emergency view without sign-in and lists every active alarm (SR-014, F-ALM-07)", async () => {
    let handler: ((e: AlarmBusEvent) => void) | undefined;
    const subscribe = (h: (e: AlarmBusEvent) => void) => {
      handler = h;
      return Promise.resolve(() => undefined);
    };
    const open = vi.fn(() => Promise.resolve({ status: "ok" as const, data: { opened: true, audited: true } }));
    renderApp(api({ breakGlassOpen: open }), subscribe);
    await screen.findByRole("form", { name: "Sign in" });
    handler?.({ type: "alarm", alarm: "a1", crewId: "c1", priority: "critical", category: "CARDIO", state: "active", title: "No heartbeat detected" });
    fireEvent.click(screen.getByRole("button", { name: /Emergency/ }));
    const view = await screen.findByRole("region", { name: "Emergency view" });
    expect(view).toHaveTextContent("Critical: No heartbeat detected");
    expect(open).toHaveBeenCalled();
  });

  it("shows why an alarm action was refused (the core decides who may act)", async () => {
    let handler: ((e: AlarmBusEvent) => void) | undefined;
    const subscribe = (h: (e: AlarmBusEvent) => void) => {
      handler = h;
      return Promise.resolve(() => undefined);
    };
    renderApp(api({ alarmAcknowledge: () => Promise.resolve(err("signInRequired")) }), subscribe);
    await screen.findByRole("form", { name: "Sign in" });
    handler?.({ type: "alarm", alarm: "a1", crewId: "c1", priority: "warning", category: "CARDIO", state: "active", title: "High heart rate at rest" });
    fireEvent.click(await screen.findByRole("button", { name: "Acknowledge" }));
    expect(await screen.findByText("Sign in first.")).toBeInTheDocument();
  });

  it("sounds alarms from the alarm bus without sign-in and stops when acknowledged (SR-015)", async () => {
    let handler: ((e: AlarmBusEvent) => void) | undefined;
    const subscribe = (h: (e: AlarmBusEvent) => void) => {
      handler = h;
      return Promise.resolve(() => undefined);
    };
    const played: string[] = [];
    let paused = 0;
    const makeAudio = (src: string): AudioLike => ({
      loop: false,
      currentTime: 0,
      play: () => {
        played.push(src);
        return Promise.resolve();
      },
      pause: () => {
        paused += 1;
      },
    });
    render(<App api={api()} subscribeAlarms={subscribe} dataEvents={noData} makeAudio={makeAudio} />);
    await waitFor(() => {
      expect(handler).toBeDefined();
    });
    handler?.({ type: "sound", alarm: "a", pattern: "high", scope: "allCrewStations", wristband: false });
    expect(played).toEqual(["/sounds/high.wav"]);
    handler?.({ type: "soundOff", alarm: "a", reason: "acknowledged" });
    expect(paused).toBe(1);
  });
});
