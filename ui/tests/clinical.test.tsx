// M8–M12 screens (CR-019, CR-015, CR-016, CR-018): Medical (overview, encounters with sign-off,
// medication, protocols, export), Checks (PVT, check-in), Environment, Ground, models, kiosk.
// Values only with source and time; NO DATA / STALE otherwise (SR-004, CLAUDE.md rule 2).
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Api } from "../src/api";
import type { CrewMember, OverviewEntry, VitalItem } from "../src/bindings";
import { Checks, CheckinForm, PvtTest } from "../src/screens/Checks";
import { Environment } from "../src/screens/Environment";
import { Ground, Models } from "../src/screens/Ground";
import { Medical } from "../src/screens/Medical";
import { KioskBoard } from "../src/screens/Windows";

const ok = <T,>(data: T) => Promise.resolve({ status: "ok" as const, data });
const T = Date.UTC(2026, 9, 10, 12, 0, 0);
const crew: CrewMember[] = [{ id: "c1", name: "Fixture Astronaut A", position: "flight engineer", mission: "TEST-1", active: true }];

const overview: OverviewEntry[] = [
  {
    crewMemberId: "c1",
    name: "Fixture Astronaut A",
    topPriority: "warning",
    vitals: [
      {
        reading: { parameter: "heart_rate", value: 72, text: null, unit: "/min", measuredAtTai: "1", measuredAtUtcMs: T, quality: "good", classification: "measured", deviceId: "d1" },
        status: "live",
      },
      {
        reading: { parameter: "spo2", value: 97, text: null, unit: "%", measuredAtTai: "1", measuredAtUtcMs: T, quality: "good", classification: "measured", deviceId: "d1" },
        status: "stale",
      },
    ],
  },
];

function api(over: Partial<Api> = {}): Api {
  return {
    crewOverview: () => ok(overview),
    encounterList: () => ok([]),
    encounterSave: vi.fn(() => ok("e1")),
    encounterSign: vi.fn(() => ok(null)),
    attachmentList: () => ok([]),
    checkinList: () => ok([]),
    medicationLog: () => ok([]),
    medicationAdd: vi.fn(() => ok("m1")),
    protocolList: () => ok([]),
    checklistList: () => ok([]),
    modelList: () => ok([]),
    authReauth: vi.fn(() => ok(null)),
    exportFhir: vi.fn(() => ok("{}")),
    exportCsv: vi.fn(() => ok("measured_at_utc\r\n")),
    pvtSubmit: vi.fn(() => ok({ medianMs: 295, lapses: 2, responses: 10 })),
    checkinSubmit: vi.fn(() => ok(null)),
    ...over,
  } as unknown as Api;
}

describe("Medical", () => {
  it("shows the crew overview with values only when live (F-CLN-01, SR-004)", async () => {
    render(<Medical api={api()} userRole="cmo" self="u1" crewId={null} crew={crew} version={0} />);
    const table = await screen.findByRole("table");
    expect(within(table).getByText("72 /min")).toBeInTheDocument();
    expect(within(table).queryByText("97 %")).not.toBeInTheDocument();
    expect(within(table).getByText("STALE")).toBeInTheDocument();
    expect(within(table).getByText("Warning")).toBeInTheDocument();
    expect(await screen.findByText("No protocol library is installed on this station.")).toBeInTheDocument();
    expect(screen.getByText("No models are installed. Model outputs are advisory and never change alarm rules.")).toBeInTheDocument();
  });

  it("saves an encounter draft and signs it after re-authentication (F-CLN-05)", async () => {
    let confirmed = false;
    const encounterSign = vi.fn(() =>
      confirmed ? ok(null) : Promise.resolve({ status: "error" as const, error: { kind: "reauthRequired" as const, message: "", retryAfterS: null } }),
    );
    const a = api({
      encounterSign,
      authReauth: vi.fn(() => {
        confirmed = true;
        return ok(null);
      }),
    });
    render(<Medical api={a} userRole="cmo" self="u1" crewId="c1" crew={crew} version={0} />);
    fireEvent.click(await screen.findByRole("button", { name: "New encounter" }));
    fireEvent.change(screen.getByLabelText("Subjective"), { target: { value: "tired" } });
    fireEvent.submit(screen.getByRole("form", { name: "New encounter" }));
    expect(await screen.findByText("Draft saved.")).toBeInTheDocument();
    expect(a.encounterSave).toHaveBeenCalledWith(null, "c1", "tired", null, null, null, []);
    fireEvent.click(screen.getByRole("button", { name: "Sign" }));
    const prompt = await screen.findByRole("form", { name: "Confirm it is you" });
    fireEvent.change(within(prompt).getByLabelText("PIN or password"), { target: { value: "135792" } });
    fireEvent.submit(prompt);
    expect(await screen.findByText("Encounter signed. It can no longer change.")).toBeInTheDocument();
    expect(encounterSign).toHaveBeenLastCalledWith("e1");
  });

  it("records a dose for the CMO only (F-CLN-08)", async () => {
    const a = api();
    const { unmount } = render(<Medical api={a} userRole="flight_surgeon" self="u2" crewId="c1" crew={crew} version={0} />);
    await screen.findByText("Medication and medical kit");
    expect(screen.queryByRole("form", { name: "Add entry" })).not.toBeInTheDocument();
    unmount();
    render(<Medical api={a} userRole="cmo" self="u1" crewId="c1" crew={crew} version={0} />);
    const form = await screen.findByRole("form", { name: "Add entry" });
    fireEvent.change(within(form).getByLabelText("Crew member"), { target: { value: "c1" } });
    fireEvent.change(within(form).getByLabelText("Item"), { target: { value: "Item X" } });
    fireEvent.change(within(form).getByLabelText("Amount"), { target: { value: "200" } });
    fireEvent.change(within(form).getByLabelText("Unit"), { target: { value: "mg" } });
    fireEvent.submit(form);
    expect(await screen.findByText("Entry recorded.")).toBeInTheDocument();
    expect(a.medicationAdd).toHaveBeenCalledWith("c1", "dose", "Item X", 200, "mg", null);
  });
});

describe("Checks", () => {
  it("runs a PVT and submits only measured reaction times (F-CLN-06)", async () => {
    vi.useFakeTimers();
    let now = 0;
    const a = api();
    render(<PvtTest api={a} timing={{ durationS: 30, minGapMs: 2000, maxGapMs: 2000, random: () => 0, now: () => now }} />);
    fireEvent.click(screen.getByRole("button", { name: "Start test" }));
    // A press before the target is a false start, not a response.
    fireEvent.click(screen.getByRole("button", { name: "Wait…" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Too soon: wait for the target.");
    for (let i = 0; i < 3; i += 1) {
      now += 2000;
      act(() => {
        vi.advanceTimersByTime(2000);
      });
      now += 250;
      fireEvent.click(screen.getByRole("button", { name: "Now!" }));
    }
    fireEvent.click(screen.getByRole("button", { name: "Stop test" }));
    vi.useRealTimers();
    await waitFor(() => {
      expect(a.pvtSubmit).toHaveBeenCalledWith([250, 250, 250]);
    });
  });

  it("submits a check-in in the chosen privacy tier (F-CLN-07)", async () => {
    const a = api();
    render(<CheckinForm api={a} />);
    fireEvent.change(screen.getByLabelText("Mood (1 very low, 5 very good)"), { target: { value: "4" } });
    fireEvent.change(screen.getByLabelText("Hours slept"), { target: { value: "6.5" } });
    fireEvent.change(screen.getByLabelText("Symptoms (one per line)"), { target: { value: "headache\n\n" } });
    fireEvent.change(screen.getByLabelText("Who may read this"), { target: { value: "medical" } });
    fireEvent.submit(screen.getByRole("form", { name: "Daily check-in" }));
    expect(await screen.findByText("Check-in saved.")).toBeInTheDocument();
    expect(a.checkinSubmit).toHaveBeenCalledWith({ mood: 4, sleepHours: 6.5, symptoms: ["headache"] }, "medical");
  });

  it("shows the next scheduled check as NO DATA without a schedule", () => {
    render(<Checks api={api()} crewId="c1" isAstronaut={false} />);
    expect(screen.getByText("NO DATA")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Start test" })).not.toBeInTheDocument();
  });
});

describe("Environment, Ground and kiosk", () => {
  const vital = (parameter: string, value: number, unit: string): VitalItem => ({
    reading: { parameter, value, text: null, unit, measuredAtTai: "1", measuredAtUtcMs: T, quality: "good", classification: "measured", deviceId: "d9" },
    status: "live",
    ageS: 1,
    deviceModel: "ECLSS_CABIN_ATMOS_TLM",
  });

  it("shows cabin values from the vehicle with their source (F-ENV-01)", async () => {
    const a = api({
      vitalsLatest: () => ok([vital("cabin_co2", 4.2, "mm[Hg]")]),
      vitalsTrend: () => ok({ points: [], baseline: null }),
    });
    render(<Environment api={a} crewId="c1" version={0} />);
    expect(await screen.findByText("4.2")).toBeInTheDocument();
    expect(screen.getByText("ECLSS_CABIN_ATMOS_TLM · 12:00:00 UTC")).toBeInTheDocument();
    expect(screen.queryByText("Vehicle data arrives once a signed vehicle table is installed and the vehicle sends it.")).not.toBeInTheDocument();
  });

  it("says when path B and the vehicle bridge are not configured (SR-015)", async () => {
    const a = api({
      groundStatus: () =>
        ok({
          pathBConfigured: false,
          groundVersion: null,
          pathBWaiting: 0,
          pathBProblem: "no signed ground configuration",
          vehicle: { running: false, version: null, problem: null, datagrams: 0, readings: 0, rejected: [], clock: null, latestOffsetMs: null },
        }),
      mdsStatus: () => ok({ configured: false, version: null, health: "not_configured", pending: 0, held: 0, problem: null }),
    });
    render(<Ground api={a} version={0} />);
    const pathB = await screen.findByRole("region", { name: "Status to the vehicle (path B)" });
    expect(within(pathB).getByText("Not configured")).toBeInTheDocument();
    expect(screen.getByText("Not receiving")).toBeInTheDocument();
    const mds = screen.getByRole("region", { name: "Medical data system (path A)" });
    expect(await within(mds).findByText("Not configured")).toBeInTheDocument();
  });

  it("shows model validation status as text, with the development badge (F-ML-01)", async () => {
    const a = api({
      modelList: () =>
        ok([
          { id: "m2", name: "Rhythm", version: "0.1", validation: "NOT_VALIDATED", status: "INVESTIGATIONAL", reason: null, developmentSigned: true, intendedUse: "", limitations: "" },
        ]),
    });
    render(<Models api={a} />);
    expect(await screen.findByText("ADVISORY · Investigational")).toBeInTheDocument();
    expect(screen.getByText("DEVELOPMENT MODEL")).toBeInTheDocument();
  });

  it("the kiosk board lists every alarm by shape and text (F-SYS-13, F-ALM-07)", () => {
    render(
      <KioskBoard
        alarms={[{ alarm: "a1", crewId: "c1", priority: "critical", category: "RESPIRATORY", state: "active", title: "Severe low blood oxygen", stages: [], sounding: true, silenced: false }]}
      />,
    );
    expect(screen.getByText("Critical: Severe low blood oxygen")).toBeInTheDocument();
  });
});
