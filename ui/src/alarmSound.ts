import type { AlarmBusEvent } from "./bindings";

/**
 * Alarm sound on this station (docs/06 §2, SR-015): plays the highest-priority pattern among the
 * alarms currently sounding, one at a time; high and medium repeat until stopped, low and
 * technical play once. Sound starts and stops only on bus events (acknowledged, silenced, ended);
 * the visual alarm is never affected by this.
 */

/** The parts of HTMLAudioElement used here (injectable for tests). */
export interface AudioLike {
  loop: boolean;
  currentTime: number;
  play(): Promise<void>;
  pause(): void;
}

const RANK: Readonly<Record<string, number>> = { high: 3, medium: 2, low: 1 };

export const SOUND_FILES: Readonly<Record<string, string>> = {
  high: "/sounds/high.wav",
  medium: "/sounds/medium.wav",
  low: "/sounds/low.wav",
};

export class AlarmSounder {
  private readonly sounding = new Map<string, string>();
  private playing: { pattern: string; audio: AudioLike } | undefined;

  constructor(private readonly make: (src: string) => AudioLike) {}

  /** The pattern playing now, if any. */
  get current(): string | undefined {
    return this.playing?.pattern;
  }

  handle(event: AlarmBusEvent): void {
    switch (event.type) {
      case "sound":
        this.sounding.set(event.alarm, event.pattern);
        break;
      case "soundOff":
      case "ended":
        this.sounding.delete(event.alarm);
        break;
      case "alarm":
      case "stage":
        return;
    }
    this.update();
  }

  private update(): void {
    let top: string | undefined;
    for (const p of this.sounding.values()) {
      if (top === undefined || (RANK[p] ?? 0) > (RANK[top] ?? 0)) top = p;
    }
    if (top === this.playing?.pattern) return;
    this.playing?.audio.pause();
    this.playing = undefined;
    const src = top === undefined ? undefined : SOUND_FILES[top];
    if (top === undefined || src === undefined) return;
    const audio = this.make(src);
    audio.loop = top !== "low";
    audio.currentTime = 0;
    this.playing = { pattern: top, audio };
    // A refused play() (e.g. autoplay policy) must not break the alarm display: forget it, so the
    // next sound event tries again.
    audio.play().catch(() => {
      if (this.playing?.audio === audio) this.playing = undefined;
    });
  }
}
