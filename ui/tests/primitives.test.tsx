// M5 primitives (docs/07 §2, F-SYS-10): level shapes, no value without a source (CLAUDE.md rule 2),
// alarm bar and escalation tracker from the alarm bus, alarm store.
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { type AlarmMap, ranked, reduce } from "../src/alarmStore";
import { AlarmBar } from "../src/components/AlarmBar";
import { LevelBadge, LevelGlyph } from "../src/components/LevelGlyph";
import { NoData, Stat } from "../src/components/Values";

const empty: AlarmMap = new Map();

function alarms(...events: Parameters<typeof reduce>[1][]): AlarmMap {
  return events.reduce(reduce, empty);
}

const raise = (alarm: string, priority: string, title: string) =>
  ({ type: "alarm", alarm, crewId: "c1", priority, category: "RESPIRATORY", state: "active", title }) as const;

describe("LevelGlyph", () => {
  it("gives every level its own shape, including the technical square", () => {
    const shapes = (["normal", "observe", "warning", "critical", "technical"] as const).map((level) => {
      const { container, unmount } = render(<LevelGlyph level={level} />);
      const svg = container.querySelector("svg");
      const first = svg?.firstElementChild;
      const shape = first?.tagName ?? "";
      const corners = (first?.getAttribute("points") ?? "").split(" ").filter(Boolean).length;
      const rotated = first?.getAttribute("transform") ?? "";
      const parts = svg?.children.length ?? 0;
      unmount();
      expect(svg?.getAttribute("aria-hidden")).toBe("true");
      return `${shape}:${String(corners)}:${rotated}:${String(parts)}`;
    });
    expect(new Set(shapes).size).toBe(5);
  });

  it("labels the level in text, never by colour alone", () => {
    render(<LevelBadge level="critical" />);
    expect(screen.getByText("Critical")).toBeInTheDocument();
  });
});

describe("values", () => {
  it("shows NO DATA, STALE and SIGNAL UNUSABLE instead of numbers", () => {
    for (const [status, text] of [["noData", "NO DATA"], ["stale", "STALE"], ["unusable", "SIGNAL UNUSABLE"]] as const) {
      const { unmount } = render(<NoData status={status} />);
      expect(screen.getByText(text)).toBeInTheDocument();
      unmount();
    }
  });

  it("shows a number only with its source and time", () => {
    const { rerender } = render(<Stat label="SpO2" reading={{ value: 97, unit: "%", at: "12:00:01 UTC", source: "Oximeter 3" }} />);
    expect(screen.getByText("97")).toBeInTheDocument();
    expect(screen.getByText("Oximeter 3 · 12:00:01 UTC")).toBeInTheDocument();
    rerender(<Stat label="SpO2" reading={{ value: 97, unit: "%", at: "12:00:01 UTC", source: "" }} />);
    expect(screen.queryByText("97")).not.toBeInTheDocument();
    expect(screen.getByText("NO DATA")).toBeInTheDocument();
    rerender(<Stat label="SpO2" reading={null} missing="stale" />);
    expect(screen.getByText("STALE")).toBeInTheDocument();
  });
});

describe("alarm store", () => {
  it("tracks state, stages, sound and silence, and ranks by priority", () => {
    const s = alarms(
      raise("w", "warning", "Low blood oxygen"),
      raise("c", "critical", "Extreme heart rate"),
      { type: "stage", alarm: "c", stage: 0, audience: "astronaut" },
      { type: "sound", alarm: "c", pattern: "high", scope: "notified", wristband: false },
      { type: "soundOff", alarm: "c", reason: "silenced" },
      { type: "stage", alarm: "ghost", stage: 0, audience: "cmo" },
    );
    const r = ranked(s);
    expect(r.map((a) => a.alarm)).toEqual(["c", "w"]);
    expect(r[0]?.stages).toEqual([{ stage: 0, audience: "astronaut" }]);
    expect(r[0]?.silenced).toBe(true);
    const after = reduce(reduce(s, { type: "soundOff", alarm: "c", reason: "acknowledged" }), { type: "ended", alarm: "w", how: "resolved" });
    expect(ranked(after).map((a) => [a.alarm, a.silenced])).toEqual([["c", false]]);
  });
});

describe("alarm bar", () => {
  it("shows the highest alarm with shape, title, state, delivered stages and actions", () => {
    const onAck = vi.fn();
    const onSilence = vi.fn();
    const s = alarms(
      raise("w", "warning", "Low blood oxygen"),
      raise("c", "critical", "Extreme heart rate"),
      { type: "stage", alarm: "c", stage: 0, audience: "astronaut" },
      { type: "stage", alarm: "c", stage: 2, audience: "sound" },
    );
    render(<AlarmBar alarms={ranked(s)} onAcknowledge={onAck} onSilence={onSilence} />);
    expect(screen.getByText("Critical: Extreme heart rate")).toBeInTheDocument();
    expect(screen.getByText("+1 more")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Escalation" })).toHaveTextContent("Astronaut Delivered");
    fireEvent.click(screen.getByRole("button", { name: "Acknowledge" }));
    fireEvent.click(screen.getByRole("button", { name: "Silence" }));
    expect(onAck).toHaveBeenCalledWith("c");
    expect(onSilence).toHaveBeenCalledWith("c");
  });

  it("says when there is no active alarm, and offers no silence for observe notices", () => {
    const { rerender } = render(<AlarmBar alarms={[]} onAcknowledge={vi.fn()} onSilence={vi.fn()} />);
    expect(screen.getByText("No active alarms")).toBeInTheDocument();
    rerender(<AlarmBar alarms={ranked(alarms(raise("o", "observe", "Possible atrial fibrillation")))} onAcknowledge={vi.fn()} onSilence={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Silence" })).not.toBeInTheDocument();
  });
});
