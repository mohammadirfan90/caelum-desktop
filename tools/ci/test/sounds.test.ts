// Alarm sounds (docs/06 §2): the bundled WAV files are exactly what the generator produces, and the
// patterns have the documented structure.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";

import { generate, OUT_DIR, PATTERNS, SAMPLE_RATE, samples } from "../../sounds/generate.ts";

describe("alarm sounds", () => {
  it("bundled files equal the generator output (reproducible, reviewable)", () => {
    for (const [file, bytes] of generate()) {
      assert.ok(readFileSync(join(OUT_DIR, file)).equals(bytes), `${file} is stale: run node tools/sounds/generate.ts`);
    }
  });

  it("has 10 pulses for high, 3 for medium, 2 for low and 1 for technical", () => {
    assert.deepEqual(
      Object.fromEntries(Object.entries(PATTERNS).map(([k, p]) => [k, p.starts.length])),
      { high: 10, medium: 3, low: 2, technical: 1 },
    );
  });

  it("keeps fundamentals in 150–1000 Hz and pulses inside each burst", () => {
    for (const p of Object.values(PATTERNS)) {
      assert.ok(p.fundamental >= 150 && p.fundamental <= 1000);
      for (const s of p.starts) assert.ok(s + p.pulse <= p.length);
    }
  });

  it("writes 16-bit mono PCM with silence between pulses", () => {
    const wav = generate().get("medium.wav");
    assert.ok(wav);
    assert.equal(wav.toString("ascii", 0, 4), "RIFF");
    assert.equal(wav.readUInt32LE(24), SAMPLE_RATE);
    const pcm = samples(PATTERNS.medium);
    const gap = Math.round(0.28 * SAMPLE_RATE); // between pulse 1 (ends 0.2 s) and 2 (starts 0.35 s)
    assert.equal(pcm[gap], 0);
    assert.ok(pcm.some((v) => Math.abs(v) > 10_000));
  });
});
