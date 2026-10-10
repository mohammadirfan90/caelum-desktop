// Alarm sounds (docs/06 §2, M4): deterministic synthesis of the bundled WAV files, so the assets
// are reproducible from source and reviewable (no downloaded or hand-made audio).
//
// Patterns follow IEC 60601-1-8 burst structure; the tone is a fundamental with four harmonics
// (the standard asks for a fundamental of 150–1000 Hz and at least four harmonics within 15 dB).
// Distinguishability is confirmed in the usability evaluation (SR-013), which may adjust them.
//
//   high:      2 × (3 + 2 pulses) = 10 pulses, the burst repeats (high-priority alarm)
//   medium:    3 pulses (medium-priority alarm)
//   low:       2 pulses (low-priority / observe notice)
//   technical: 1 pulse (technical advisory)
//
// Usage: node tools/sounds/generate.ts [--check]   (writes ui/public/sounds/*.wav)
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const SAMPLE_RATE = 22_050;

export interface Pattern {
  /** Fundamental frequency, Hz. */
  readonly fundamental: number;
  /** Pulse duration, s. */
  readonly pulse: number;
  /** Pulse start times within one burst, s. */
  readonly starts: readonly number[];
  /** Total length of one burst including the silence after it, s. */
  readonly length: number;
}

export const PATTERNS: Readonly<Record<"high" | "medium" | "low" | "technical", Pattern>> = {
  // 3 pulses, gap, 2 pulses; again; then the pause before the burst repeats.
  high: { fundamental: 880, pulse: 0.15, starts: [0, 0.25, 0.5, 1.0, 1.25, 2.25, 2.5, 2.75, 3.25, 3.5], length: 6 },
  medium: { fundamental: 660, pulse: 0.2, starts: [0, 0.35, 0.7], length: 3 },
  low: { fundamental: 520, pulse: 0.25, starts: [0, 0.45], length: 1.2 },
  technical: { fundamental: 440, pulse: 0.3, starts: [0], length: 0.8 },
};

/** Harmonic amplitudes (fundamental first), all within 15 dB of the strongest. */
const HARMONICS = [1, 0.7, 0.5, 0.35, 0.25];

/** 16-bit mono PCM samples of one burst. */
export function samples(p: Pattern): Int16Array {
  const n = Math.round(p.length * SAMPLE_RATE);
  const out = new Int16Array(n);
  const norm = HARMONICS.reduce((a, b) => a + b, 0);
  const ramp = 0.01; // 10 ms rise and fall: no clicks
  for (const start of p.starts) {
    const s0 = Math.round(start * SAMPLE_RATE);
    const len = Math.round(p.pulse * SAMPLE_RATE);
    for (let i = 0; i < len && s0 + i < n; i += 1) {
      const t = i / SAMPLE_RATE;
      const env = Math.min(1, t / ramp, (p.pulse - t) / ramp);
      let v = 0;
      HARMONICS.forEach((a, h) => {
        v += a * Math.sin(2 * Math.PI * p.fundamental * (h + 1) * t);
      });
      out[s0 + i] = Math.round((v / norm) * env * 0.8 * 32_767);
    }
  }
  return out;
}

/** A RIFF/WAVE file for 16-bit mono PCM. */
export function wav(pcm: Int16Array): Buffer {
  const data = Buffer.alloc(pcm.length * 2);
  pcm.forEach((v, i) => data.writeInt16LE(v, i * 2));
  const header = Buffer.alloc(44);
  header.write("RIFF", 0, "ascii");
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVE", 8, "ascii");
  header.write("fmt ", 12, "ascii");
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20); // PCM
  header.writeUInt16LE(1, 22); // mono
  header.writeUInt32LE(SAMPLE_RATE, 24);
  header.writeUInt32LE(SAMPLE_RATE * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36, "ascii");
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

export const OUT_DIR = fileURLToPath(new URL("../../ui/public/sounds", import.meta.url));

export function generate(): Map<string, Buffer> {
  return new Map(Object.entries(PATTERNS).map(([name, p]) => [`${name}.wav`, wav(samples(p))]));
}

if (process.argv[1] !== undefined && fileURLToPath(import.meta.url) === process.argv[1]) {
  const check = process.argv.includes("--check");
  if (!existsSync(OUT_DIR)) mkdirSync(OUT_DIR, { recursive: true });
  let stale = 0;
  for (const [file, bytes] of generate()) {
    const path = join(OUT_DIR, file);
    const same = existsSync(path) && readFileSync(path).equals(bytes);
    if (check) {
      if (!same) {
        console.error(`sounds: ${file} differs from the generator; run node tools/sounds/generate.ts`);
        stale += 1;
      }
    } else {
      writeFileSync(path, bytes);
    }
  }
  if (stale > 0) process.exit(1);
  console.log(`sounds: ${check ? "checked" : "wrote"} ${String(generate().size)} files in ui/public/sounds`);
}
