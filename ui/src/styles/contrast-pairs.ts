/**
 * Every token pair the UI uses for text or for UI elements (glyphs, control borders, focus
 * rings). This list is the contract checked by `tests/contrast.test.ts` in every theme:
 * text >= 4.5:1, UI elements >= 3:1 (docs/07_UI_SPEC.md §1, CR-002 D5, F-SYS-10).
 *
 * Add a pair here before using a new foreground/background combination. `--warning` never
 * appears as a text foreground: it is a fill, with `--on-warning` text on it.
 * Control borders use `--ink-3` (no separate token).
 */
export type ContrastUse = "text" | "ui";

export interface ContrastPair {
  readonly fg: string;
  readonly bg: string;
  readonly use: ContrastUse;
}

const SURFACES = ["canvas", "surface", "raised", "inset"] as const;

const textOnEverySurface = (fg: string): ContrastPair[] =>
  SURFACES.map((bg) => ({ fg, bg, use: "text" }));

const uiOnEverySurface = (fg: string): ContrastPair[] =>
  SURFACES.map((bg) => ({ fg, bg, use: "ui" }));

export const CONTRAST_PAIRS: readonly ContrastPair[] = [
  // Body text and secondary text on every surface.
  ...textOnEverySurface("ink"),
  ...textOnEverySurface("ink-2"),
  ...textOnEverySurface("ink-3"),
  ...textOnEverySurface("warning-ink"),
  ...textOnEverySurface("signal"),
  // Text on filled backgrounds.
  { fg: "on-nav", bg: "nav", use: "text" },
  { fg: "on-nav-2", bg: "nav", use: "text" },
  { fg: "canvas", bg: "ink", use: "text" }, // .btn-ink, ::selection
  { fg: "signal", bg: "signal-soft", use: "text" },
  { fg: "on-warning", bg: "warning", use: "text" },
  { fg: "on-alarm", bg: "alarm-high", use: "text" },
  { fg: "on-alarm", bg: "alarm-medium", use: "text" },
  // UI elements: focus ring (ink), control borders (ink-3), level glyphs (alarm tokens).
  ...uiOnEverySurface("alarm-high"),
  ...uiOnEverySurface("alarm-medium"),
  ...uiOnEverySurface("signal"),
];

/** The themes defined in tokens.css; light is `:root`, the others override it. */
export const THEMES = ["light", "dark", "high-contrast"] as const;
export type Theme = (typeof THEMES)[number];
