import { strings } from "../strings";

/** CAELUM levels (docs/06 §2); `technical` is the advisory square with "!". */
export type Level = "normal" | "observe" | "warning" | "critical" | "technical";

/**
 * The level shape: circle (normal), diamond (observe), triangle (warning), octagon (critical),
 * square with "!" (technical). Shape and text carry the meaning; colour only adds to it.
 */
export function LevelGlyph({ level, size = 20 }: { readonly level: Level; readonly size?: number }) {
  const common = { width: size, height: size, viewBox: "0 0 20 20", "aria-hidden": true as const, focusable: "false" as const };
  switch (level) {
    case "normal":
      return (
        <svg {...common} data-level="normal">
          <circle cx="10" cy="10" r="6" fill="var(--ink-3)" />
        </svg>
      );
    case "observe":
      return (
        <svg {...common} data-level="observe">
          <rect x="5" y="5" width="10" height="10" transform="rotate(45 10 10)" fill="none" stroke="var(--ink)" strokeWidth="2" />
        </svg>
      );
    case "warning":
      return (
        <svg {...common} data-level="warning">
          <polygon points="10,2 19,18 1,18" fill="var(--warning)" stroke="var(--ink)" strokeWidth="1.5" />
        </svg>
      );
    case "critical":
      return (
        <svg {...common} data-level="critical">
          <polygon points="6,1 14,1 19,6 19,14 14,19 6,19 1,14 1,6" fill="var(--alarm-high)" stroke="var(--ink)" strokeWidth="1" />
        </svg>
      );
    case "technical":
      return (
        <svg {...common} data-level="technical">
          <rect x="2" y="2" width="16" height="16" rx="2" fill="none" stroke="var(--ink)" strokeWidth="2" />
          <rect x="9" y="5" width="2" height="7" fill="var(--ink)" />
          <rect x="9" y="13.5" width="2" height="2" fill="var(--ink)" />
        </svg>
      );
  }
}

/** Glyph with its level name as text. */
export function LevelBadge({ level }: { readonly level: Level }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-sm font-semibold">
      <LevelGlyph level={level} size={16} />
      {strings.level[level]}
    </span>
  );
}
