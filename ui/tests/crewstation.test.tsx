// M11 crew stations (F-SYS-05, F-SYS-13, SR-014; CR-017): a secondary display signs users in on
// the primary and lists what the primary streams; a kiosk shows the crew-wide view without
// sign-in; without a link the station shows NO DATA, never the last known state.
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { CrewLanView } from "../src/bindings";
import { CrewStation, type CrewStationApi } from "../src/screens/CrewStation";

const ok = <T,>(data: T) => Promise.resolve({ status: "ok" as const, data });
const view = (over: Partial<CrewLanView>): CrewLanView => ({
  configured: true,
  kiosk: false,
  connected: true,
  signedIn: null,
  problem: null,
  alarms: [],
  ...over,
});
const alarm = { alarmId: "a1", crewId: "c1", priority: "critical", category: "oxygenation", state: "active", stage: 2, title: "Severe low blood oxygen" };

function api(v: CrewLanView, over: Partial<CrewStationApi> = {}): CrewStationApi {
  return { crewLanStatus: () => ok(v), crewLanSignIn: vi.fn(() => ok("cmo.b")), crewLanSignOut: vi.fn(() => ok(null)), ...over };
}
const noEvents = () => () => undefined;

describe("crew station", () => {
  it("asks a secondary display to sign in on the primary", async () => {
    const a = api(view({ problem: "sign in to see alarms", connected: false }));
    render(<CrewStation api={a} onChange={noEvents} />);
    const form = await screen.findByRole("form", { name: "Sign in on the primary" });
    fireEvent.change(screen.getByLabelText("Sign-in name"), { target: { value: "cmo.b" } });
    fireEvent.change(screen.getByLabelText("PIN or password"), { target: { value: "135792" } });
    fireEvent.submit(form);
    expect(a.crewLanSignIn).toHaveBeenCalledWith("cmo.b", "135792");
    expect(screen.getByText("NO DATA")).toBeInTheDocument();
  });

  it("lists the alarms the primary streams, by shape and text", async () => {
    render(<CrewStation api={api(view({ signedIn: "cmo.b", alarms: [alarm] }))} onChange={noEvents} />);
    expect(await screen.findByText("Critical: Severe low blood oxygen · active, stage 2")).toBeInTheDocument();
    expect(screen.getByText("Signed in on the primary as cmo.b.")).toBeInTheDocument();
  });

  it("a kiosk shows the crew-wide view without sign-in (SR-014), and NO DATA without a link", async () => {
    const { unmount } = render(<CrewStation api={api(view({ kiosk: true, alarms: [{ ...alarm, title: null }] }))} onChange={noEvents} />);
    expect(await screen.findByText("Critical: oxygenation · active, stage 2")).toBeInTheDocument();
    expect(screen.queryByRole("form")).not.toBeInTheDocument();
    unmount();
    render(<CrewStation api={api(view({ kiosk: true, connected: false, problem: "no primary answers on the crew LAN" }))} onChange={noEvents} />);
    expect(await screen.findByText("no primary answers on the crew LAN")).toBeInTheDocument();
    expect(screen.getByText("NO DATA")).toBeInTheDocument();
  });
});
