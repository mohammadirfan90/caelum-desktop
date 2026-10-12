// Ground console profile (F-GND-08; CR-025): the flight surgeon sees downlinked summaries, event
// packets and the private medical conference record; BME and MCC see OPS records only; nothing
// received shows NO DATA.
/* eslint-disable jsx-a11y/aria-role -- `role` is the Console component prop for the station user role, not an ARIA role */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Api } from "../src/api";
import { Console } from "../src/screens/Console";

const ok = <T,>(data: T) => Promise.resolve({ status: "ok" as const, data });
const T = Date.UTC(2026, 9, 10, 12, 0, 0);

function api(over: Partial<Api> = {}): Api {
  return {
    consoleOps: () =>
      ok([JSON.stringify({ type: "ops_alarm", crewId: "c1", priority: "critical", category: "CARDIO", state: "active", at: "2026-10-10T12:00:00Z" })]),
    consoleFiles: () => ok([{ id: "f1", name: "event-a1.json", kind: "event_packet", receivedUtcMs: T }]),
    consoleFile: () => ok('{"format":"caelum-event-packet/1"}'),
    consoleSummaries: () =>
      ok([{ parameter: "heart_rate", windowEndUtcMs: T, quality: "good", min: 58, mean: 64.5, max: 71, state: null, count: 60 }]),
    pmcList: () => ok([]),
    pmcSave: vi.fn(() => ok("p1")),
    pmcSign: vi.fn(() => ok(null)),
    authReauth: vi.fn(() => ok(null)),
    ...over,
  } as unknown as Api;
}

describe("ground console", () => {
  it("shows the flight surgeon summaries, event packets and the PMC record (F-GND-08)", async () => {
    const a = api();
    render(<Console api={a} role="flight_surgeon" crewId="c1" />);
    expect(await screen.findByText("58 / 64.5 / 71")).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Open event-a1.json" }));
    expect(await screen.findByText('{"format":"caelum-event-packet/1"}')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Notes"), { target: { value: "Discussed sleep." } });
    fireEvent.submit(screen.getByRole("form", { name: "New conference record" }));
    expect(await screen.findByText("Draft saved.")).toBeInTheDocument();
    expect(screen.getByText("2026-10-10T12:00:00Z · critical · CARDIO · active")).toBeInTheDocument();
  });

  it("shows BME and MCC the OPS records only, and NO DATA when nothing arrived", async () => {
    render(<Console api={api({ consoleOps: () => ok([]) })} role="mcc" crewId="c1" />);
    expect(await screen.findByText("NO DATA")).toBeInTheDocument();
    expect(screen.queryByText("Event packets received")).not.toBeInTheDocument();
    expect(screen.queryByText("Private medical conference record")).not.toBeInTheDocument();
  });
});
