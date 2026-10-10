// Alarm sound on the station (docs/06 §2, SR-015): the highest-priority sounding alarm plays, high
// and medium repeat, sound stops on acknowledge/silence/end, and nothing depends on a wristband.
// @vitest-environment node
import { describe, expect, it } from "vitest";

import { AlarmSounder, type AudioLike, SOUND_FILES } from "../src/alarmSound";

class FakeAudio implements AudioLike {
  loop = false;
  currentTime = 5;
  playing = false;
  constructor(readonly src: string, private readonly refuse = false) {}
  play(): Promise<void> {
    if (this.refuse) return Promise.reject(new Error("autoplay"));
    this.playing = true;
    return Promise.resolve();
  }
  pause(): void {
    this.playing = false;
  }
}

function setup(refuseFirst = false) {
  const made: FakeAudio[] = [];
  const sounder = new AlarmSounder((src) => {
    const a = new FakeAudio(src, refuseFirst && made.length === 0);
    made.push(a);
    return a;
  });
  return { sounder, made };
}

const sound = (alarm: string, pattern: string, wristband = false) =>
  ({ type: "sound", alarm, pattern, scope: "allCrewStations", wristband }) as const;

describe("alarm sounder", () => {
  it("plays the high pattern on repeat for a critical alarm, without needing a wristband", () => {
    const { sounder, made } = setup();
    sounder.handle(sound("a", "high", false));
    expect(sounder.current).toBe("high");
    expect(made[0]?.src).toBe(SOUND_FILES.high);
    expect(made[0]?.loop).toBe(true);
    expect(made[0]?.currentTime).toBe(0);
    expect(made[0]?.playing).toBe(true);
  });

  it("always plays the highest priority among sounding alarms", () => {
    const { sounder, made } = setup();
    sounder.handle(sound("w", "medium"));
    sounder.handle(sound("c", "high"));
    expect(sounder.current).toBe("high");
    expect(made[0]?.playing).toBe(false);
    sounder.handle({ type: "soundOff", alarm: "c", reason: "acknowledged" });
    expect(sounder.current).toBe("medium");
    sounder.handle({ type: "ended", alarm: "w", how: "resolved" });
    expect(sounder.current).toBeUndefined();
    expect(made.every((a) => !a.playing)).toBe(true);
  });

  it("plays low once, and ignores alarm and stage events", () => {
    const { sounder, made } = setup();
    sounder.handle({ type: "alarm", alarm: "o", crewId: "c", priority: "observe", category: "CARDIO", state: "active", title: "t" });
    sounder.handle({ type: "stage", alarm: "o", stage: 0, audience: "astronaut" });
    expect(made).toHaveLength(0);
    sounder.handle(sound("o", "low"));
    expect(made[0]?.loop).toBe(false);
  });

  it("retries after a refused play on the next sound event", async () => {
    const { sounder, made } = setup(true);
    sounder.handle(sound("a", "high"));
    await Promise.resolve();
    await Promise.resolve();
    expect(sounder.current).toBeUndefined();
    sounder.handle(sound("a", "high"));
    expect(made).toHaveLength(2);
    expect(made[1]?.playing).toBe(true);
  });
});
