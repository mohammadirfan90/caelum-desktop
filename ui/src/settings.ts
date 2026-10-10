/**
 * Station display preferences (docs/07, F-SYS-10): theme (light, dark, high contrast), text size
 * up to 200 %, glove mode and reduced motion. Kept per station in browser storage, which can be
 * missing or refused: then the defaults apply and nothing breaks.
 */
export type Theme = "light" | "dark" | "high-contrast";

export interface Prefs {
  readonly theme: Theme;
  /** Text size in percent: 100 to 200. */
  readonly textScale: number;
  readonly glove: boolean;
  readonly reducedMotion: boolean;
}

export const TEXT_SCALES = [100, 125, 150, 175, 200] as const;

export const DEFAULT_PREFS: Prefs = { theme: "light", textScale: 100, glove: false, reducedMotion: false };

const KEY = "caelum.prefs";

function valid(p: unknown): p is Prefs {
  if (typeof p !== "object" || p === null) return false;
  const o = p as Record<string, unknown>;
  return (
    (o.theme === "light" || o.theme === "dark" || o.theme === "high-contrast") &&
    typeof o.textScale === "number" &&
    (TEXT_SCALES as readonly number[]).includes(o.textScale) &&
    typeof o.glove === "boolean" &&
    typeof o.reducedMotion === "boolean"
  );
}

export function loadPrefs(): Prefs {
  try {
    const raw = window.localStorage.getItem(KEY);
    const parsed: unknown = raw === null ? null : JSON.parse(raw);
    return valid(parsed) ? parsed : DEFAULT_PREFS;
  } catch {
    return DEFAULT_PREFS;
  }
}

export function savePrefs(p: Prefs): void {
  try {
    window.localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    // Storage refused: the preference applies until the window closes.
  }
}

/** Applies preferences to the document root. */
export function applyPrefs(p: Prefs, root: HTMLElement = document.documentElement): void {
  root.dataset.theme = p.theme;
  root.style.fontSize = `${String(p.textScale)}%`;
  root.dataset.glove = p.glove ? "on" : "off";
  root.dataset.motion = p.reducedMotion ? "reduce" : "normal";
}
