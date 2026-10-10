// M0 window (F-SYS-06): shows the version reported by the core, and never a made-up one
// (CLAUDE.md rule 2, R-CLN-02).
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { App } from "../src/App";
import type { AudioLike } from "../src/alarmSound";
import type { AlarmBusEvent, AppInfo } from "../src/bindings";

const info: AppInfo = { productName: "CAELUM", version: "0.1.0" };

/** No alarm bus in these tests. */
const noAlarms = () => Promise.resolve(() => undefined);

/** A verified, flight-signed ruleset: no banner, so these tests see only the window itself. */
const quietRuleset = () =>
  Promise.resolve({
    status: "ok" as const,
    data: { state: "verified" as const, version: "1.0.0", development: false, reason: null },
  });

describe("App", () => {
  it("shows a loading status until the core answers, with no version", () => {
    render(<App subscribeAlarms={noAlarms} loadRuleset={quietRuleset} loadAppInfo={() => new Promise<AppInfo>(() => undefined)} />);
    expect(screen.getByRole("status")).toHaveTextContent("Starting…");
    expect(screen.queryByText(/Version \d/)).not.toBeInTheDocument();
  });

  it("shows the product name and the version reported over IPC", async () => {
    render(<App subscribeAlarms={noAlarms} loadRuleset={quietRuleset} loadAppInfo={() => Promise.resolve(info)} />);
    expect(screen.getByRole("heading", { level: 1, name: "CAELUM" })).toBeInTheDocument();
    expect(await screen.findByText("Version 0.1.0")).toBeInTheDocument();
  });

  it("shows the failure, and no version, when the IPC call fails", async () => {
    render(<App subscribeAlarms={noAlarms} loadRuleset={quietRuleset} loadAppInfo={() => Promise.reject(new Error("IPC unavailable"))} />);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Version unavailable");
    expect(alert).toHaveTextContent("IPC unavailable");
    expect(screen.queryByText(/\d+\.\d+\.\d+/)).not.toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("reports non-Error rejections as text instead of crashing", async () => {
    // Tauri's invoke() rejects with plain strings (e.g. permission denials), so this case is real.
    // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
    render(<App subscribeAlarms={noAlarms} loadRuleset={quietRuleset} loadAppInfo={() => Promise.reject("command app_info not allowed")} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("command app_info not allowed");
  });

  it("uses a main landmark and a labelled section", () => {
    render(<App subscribeAlarms={noAlarms} loadRuleset={quietRuleset} loadAppInfo={() => Promise.resolve(info)} />);
    expect(screen.getByRole("main")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "CAELUM" })).toBeInTheDocument();
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
    render(<App subscribeAlarms={subscribe} makeAudio={makeAudio} loadRuleset={quietRuleset} loadAppInfo={() => Promise.resolve(info)} />);
    await waitFor(() => {
      expect(handler).toBeDefined();
    });
    handler?.({ type: "sound", alarm: "a", pattern: "high", scope: "allCrewStations", wristband: false });
    expect(played).toEqual(["/sounds/high.wav"]);
    handler?.({ type: "soundOff", alarm: "a", reason: "acknowledged" });
    expect(paused).toBe(1);
  });
});
