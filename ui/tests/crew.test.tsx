// M6 crew screens (docs/07 §4; CR-012 §4): Home, Body, Alerts, Devices, Audit on data from the
// core. Values only with source and time; STALE / SIGNAL UNUSABLE / NO DATA otherwise (SR-004,
// CLAUDE.md rule 2); MCC sees alarm status only (docs/06 §3a); F-ALM-05 actions; F-ALM-09 report;
// F-ACQ-09 assignment with re-authentication; F-SYS-03 broken audit chain shown.
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Api, DataEvents } from "../src/api";
import type { AlarmEntry, AuditStatus, DeviceEntry, SessionInfo, VitalItem } from "../src/bindings";
import { Alerts } from "../src/screens/Alerts";
import { Audit } from "../src/screens/Audit";
import { Body, Home } from "../src/screens/Crew";
import { Devices } from "../src/screens/Devices";
import { Shell } from "../src/screens/Shell";
import { DEFAULT_PREFS } from "../src/settings";

const ok = <T,>(data: T) => Promise.resolve({ status: "ok" as const, data });
const T = Date.UTC(2026, 9, 10, 12, 0, 1);

function vital(parameter: string, value: number, unit: string, status: VitalItem["status"], ageS = 2): VitalItem {
  return {
    reading: { parameter, value, text: null, unit, measuredAtTai: "1", measuredAtUtcMs: T, quality: status === "unusable" ? "unusable" : "good", classification: "measured", deviceId: "d1" },
    status,
    ageS,
    deviceModel: "Oximeter X",
  };
}

const alarm = (over: Partial<AlarmEntry> = {}): AlarmEntry => ({
  id: "a1",
  crewMemberId: "c1",
  kind: "physiological",
  category: "RESPIRATORY",
  priority: "critical",
  state: "active",
  title: "Severe low blood oxygen",
  ruleId: "spo2.critical",
  createdAtTai: "1",
  createdAtUtcMs: T,
  acknowledgedAtUtcMs: null,
  resolvedAtUtcMs: null,
  acknowledgedAtTai: null,
  groundAcknowledgedAtTai: null,
  silencedUntilTai: null,
  resolvedAtTai: null,
  escalationStage: 3,
  opsOnly: false,
  ...over,
});

const crew = [{ id: "c1", name: "Fixture Astronaut A", position: "flight engineer", mission: "TEST-1", active: true }];

function api(over: Partial<Api> = {}): Api {
  return {
    vitalsLatest: () => ok([vital("spo2", 97, "%", "live"), vital("heart_rate", 72, "/min", "stale", 900), vital("respiration_rate", 14, "/min", "unusable")]),
    vitalsTrend: () => ok({ points: [], baseline: null }),
    waveformRecent: () => ok([]),
    alarmList: () => ok([alarm()]),
    alarmDetail: () =>
      ok({
        alarm: alarm(),
        rulesetVersion: "1.0.0-draft.3",
        triggerValue: 82,
        why: JSON.stringify({ rule: "spo2.critical", held_s: 15 }),
        action: "Check the sensor, then oxygen",
        timeline: [
          { audience: "astronaut", stage: 0, atTai: "1", atUtcMs: T },
          { audience: "cmo", stage: 1, atTai: "1", atUtcMs: T },
        ],
      }),
    alarmReport: () => ok({ fromTai: "0", toTai: "1", history: [], rules: [], perCrew: [] }),
    alarmAcknowledge: vi.fn(() => ok("onboard" as const)),
    alarmSilence: vi.fn(() => ok(120)),
    alarmEscalate: vi.fn(() => ok(null)),
    crewList: () => ok(crew),
    authReauth: vi.fn(() => ok(null)),
    ...over,
  } as unknown as Api;
}

describe("Home and Body", () => {
  // R-CLN-01: vital signs with source device, measurement time and quality.
  it("shows values with source and time, and STALE / SIGNAL UNUSABLE / NO DATA otherwise (SR-004)", async () => {
    render(<Home ctx={{ api: api(), crewId: "c1", version: 0, alarms: [] }} />);
    expect(await screen.findByText("97")).toBeInTheDocument();
    expect(screen.getByText("Oximeter X · 12:00:01 UTC")).toBeInTheDocument();
    expect(screen.getAllByText("STALE").length).toBeGreaterThan(0);
    expect(screen.queryByText("72")).not.toBeInTheDocument();
    expect(screen.getAllByText("SIGNAL UNUSABLE").length).toBeGreaterThan(0);
    expect(screen.getAllByText("NO DATA").length).toBeGreaterThan(0);
    expect(screen.getByText("No active alarms for this crew member.")).toBeInTheDocument();
  });

  it("asks for a crew member instead of showing anyone's data by default", () => {
    render(<Home ctx={{ api: api(), crewId: null, version: 0, alarms: [] }} />);
    expect(screen.getByText("Choose a crew member in the bar above to see their data.")).toBeInTheDocument();
  });

  it("draws the trend and says when there is no personal baseline; ECG is NO DATA without chunks", async () => {
    const vitalsTrend = vi.fn(() =>
      ok({
        points: [
          { parameter: "heart_rate", value: 70, text: null, unit: "/min", measuredAtTai: "1", measuredAtUtcMs: T, quality: "good", classification: "measured", deviceId: "d1" },
          { parameter: "heart_rate", value: 74, text: null, unit: "/min", measuredAtTai: "2", measuredAtUtcMs: T + 60_000, quality: "good", classification: "measured", deviceId: "d1" },
        ],
        baseline: null,
      }),
    );
    render(<Body ctx={{ api: api({ vitalsTrend }), crewId: "c1", version: 0, alarms: [] }} />);
    expect(await screen.findByRole("img", { name: "Heart rate: 2 readings, latest 74 /min 12:01:01 UTC" })).toBeInTheDocument();
    expect(screen.getByText("No personal baseline yet.")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Window"), { target: { value: "86400" } });
    await waitFor(() => {
      expect(vitalsTrend).toHaveBeenLastCalledWith("c1", "heart_rate", 86400);
    });
  });
});

describe("Alerts", () => {
  it("lists alarms, opens the why and the escalation timeline, and acknowledges (F-ALM-05)", async () => {
    const a = api();
    render(<Alerts api={a} userRole="cmo" crew={crew} version={0} />);
    fireEvent.click(await screen.findByRole("button", { name: "Show details: Severe low blood oxygen" }));
    const detail = await screen.findByRole("region", { name: "Critical: Severe low blood oxygen" });
    expect(within(detail).getByText("held_s")).toBeInTheDocument();
    expect(within(detail).getByText("Check the sensor, then oxygen")).toBeInTheDocument();
    expect(within(detail).getByText("Medical officer · 12:00:01 UTC")).toBeInTheDocument();
    fireEvent.click(within(detail).getByRole("button", { name: "Acknowledge" }));
    expect(a.alarmAcknowledge).toHaveBeenCalledWith("a1");
    expect(await screen.findByRole("region", { name: "Alarm history and fatigue report" })).toBeInTheDocument();
  });

  it("shows MCC the status only: no title, no details, no report (docs/06 §3a)", async () => {
    const opsApi = api({ alarmList: () => ok([alarm({ title: null, ruleId: "", opsOnly: true })]) });
    render(<Alerts api={opsApi} userRole="mcc" crew={[]} version={0} />);
    expect(await screen.findByText("Status only")).toBeInTheDocument();
    expect(screen.getByText("Breathing")).toBeInTheDocument();
    expect(screen.queryByText("Severe low blood oxygen")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Show details/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Alarm history and fatigue report" })).not.toBeInTheDocument();
  });

  // R-ALM-04: technical alarms are named as technical, apart from physiological ones.
  it("names technical alarms from the catalog", async () => {
    render(<Alerts api={api({ alarmList: () => ok([alarm({ kind: "technical", title: null, ruleId: "technical.low_battery.d1", priority: "advisory" })]) })} userRole="bme" crew={crew} version={0} />);
    expect(await screen.findByText("Device battery low")).toBeInTheDocument();
  });
});

describe("Devices", () => {
  const device: DeviceEntry = {
    id: "d1", adapter: "ble-sig", model: "Oximeter X", vendor: "unknown", serial: "AA:BB:CC:DD:EE:FF", firmware: null, crewMemberId: null,
    deviceUser: null, status: "connected", lastSeenTai: "1", lastSeenUtcMs: T, clockOffsetMs: null, batteryPct: 81,
  };

  it("assigns after re-authentication and pairs a scanned device (F-ACQ-01, F-ACQ-09)", async () => {
    let confirmed = false;
    const deviceAssign = vi.fn(() => (confirmed ? ok(null) : Promise.resolve({ status: "error" as const, error: { kind: "reauthRequired" as const, message: "", retryAfterS: null } })));
    const authReauth = vi.fn(() => {
      confirmed = true;
      return ok(null);
    });
    const devicePair = vi.fn(() => ok({ result: "registered" as const, deviceId: "d2" }));
    const a = api({
      deviceList: () => ok([device]),
      deviceAssign,
      authReauth,
      deviceScan: () => ok([{ address: "TE:00:00:00:00:01", name: "Test equipment oximeter", rssi: null, services: [0x1822], emulated: true }]),
      devicePair,
    });
    render(<Devices api={a} userRole="cmo" crew={crew} version={0} />);
    expect(await screen.findByText("81 %")).toBeInTheDocument();
    expect(screen.getByText("Connected")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Assign Oximeter X to"), { target: { value: "c1" } });
    fireEvent.click(screen.getByRole("button", { name: "Assign Oximeter X" }));
    const prompt = await screen.findByRole("form", { name: "Confirm it is you" });
    fireEvent.change(within(prompt).getByLabelText("PIN or password"), { target: { value: "135792" } });
    fireEvent.submit(prompt);
    expect(await screen.findByText("Oximeter X assigned to Fixture Astronaut A.")).toBeInTheDocument();
    expect(deviceAssign).toHaveBeenLastCalledWith("d1", "c1", null);

    fireEvent.click(screen.getByRole("button", { name: "Scan for devices" }));
    fireEvent.click(await screen.findByRole("button", { name: "Pair Test equipment oximeter" }));
    expect(await screen.findByText("Test equipment oximeter paired. Assign it to a crew member.")).toBeInTheDocument();
    expect(devicePair).toHaveBeenCalledWith("TE:00:00:00:00:01", "Test equipment oximeter", [0x1822]);
  });

  it("shows an astronaut their devices without management controls", async () => {
    render(<Devices api={api({ deviceList: () => ok([{ ...device, crewMemberId: "c1" }]) })} userRole="astronaut" crew={crew} version={0} />);
    expect(await screen.findByText("Fixture Astronaut A")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Scan for devices" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /assignment/ })).not.toBeInTheDocument();
  });
});

describe("Audit", () => {
  it("shows a broken chain and where (F-SYS-03)", async () => {
    const broken: AuditStatus = { entries: "12", intact: false, problems: [{ seq: "12", kind: "hashMismatch" }] };
    render(<Audit api={api({ auditVerify: () => ok(broken), auditList: () => ok([]) })} version={0} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Audit chain BROKEN: problems found in 12 entries checked.");
    expect(screen.getByText("Entry 12: content changed")).toBeInTheDocument();
  });

  it("confirms an intact chain and lists entries newest first", async () => {
    const items = [
      { seq: "1", id: "e1", atTai: "1", atUtcMs: T, actor: "system", role: "system", action: "key.create", target: null, details: "{}" },
      { seq: "2", id: "e2", atTai: "2", atUtcMs: T + 1000, actor: "u", role: "administrator", action: "config.apply", target: "crew", details: "{}" },
    ];
    render(<Audit api={api({ auditVerify: () => ok({ entries: "2", intact: true, problems: [] }), auditList: () => ok(items) })} version={0} />);
    expect(await screen.findByRole("status")).toHaveTextContent("Audit chain intact: 2 entries checked");
    const rows = screen.getAllByRole("row");
    expect(rows[1]).toHaveTextContent("config.apply");
  });
});

describe("Shell crew selection and live updates", () => {
  const cmo: SessionInfo = { userId: "u1", username: "cmo.b", role: "cmo", crewMemberId: "c2", locked: false };

  it("lets the CMO choose a crew member and reloads values when new readings arrive", async () => {
    let notify: ((crewId: string) => void) | undefined;
    const events: DataEvents = {
      onObservations: (h) => {
        notify = h;
        return Promise.resolve(() => undefined);
      },
      onDevices: () => Promise.resolve(() => undefined),
    };
    const vitalsLatest = vi.fn(() => ok([vital("spo2", 97, "%", "live")]));
    render(<Shell api={api({ vitalsLatest })} session={cmo} prefs={DEFAULT_PREFS} onPrefs={() => undefined} onSignOut={() => undefined} events={events} />);
    expect(screen.getByText("Choose a crew member in the bar above to see their data.")).toBeInTheDocument();
    fireEvent.change(await screen.findByRole("combobox", { name: "Crew member" }), { target: { value: "c1" } });
    expect(await screen.findByText("97")).toBeInTheDocument();
    expect(vitalsLatest).toHaveBeenCalledTimes(1);
    notify?.("c1");
    await waitFor(() => {
      expect(vitalsLatest).toHaveBeenCalledTimes(2);
    });
    notify?.("someone-else");
    expect(vitalsLatest).toHaveBeenCalledTimes(2);
  });
});
