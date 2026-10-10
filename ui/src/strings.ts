/**
 * The localisation catalog (F-SYS-11): every user-visible string lives here; components never
 * contain literal text (checked by `tests/i18n.test.ts`). English is the only locale in v1.0;
 * another locale is a second object of type `Messages`.
 */
export const en = {
  productName: "CAELUM",
  tagline: "Onboard crew health monitoring",
  loading: "Starting…",
  version: (version: string) => `Version ${version}`,
  versionUnavailable: "Version unavailable",
  coreNoResponse: (reason: string) => `The application core did not respond: ${reason}`,

  // Ruleset banner (SR-009)
  rulesetLabel: "Alarm ruleset",
  developmentRuleset: (version: string) => `DEVELOPMENT RULESET ${version}: not for operational use`,
  rulesetFallback: (version: string) => `The installed alarm ruleset was rejected; ruleset ${version} is in use`,
  rulesetSafeState: "Physiological alarm rules are not running: no verified ruleset",
  rulesetUnknown: "Alarm ruleset status unavailable",

  // Levels (docs/06 §2)
  level: {
    normal: "Normal",
    observe: "Observe",
    warning: "Warning",
    critical: "Critical",
    technical: "Technical",
  },

  // Missing values (CLAUDE.md rule 2, SR-004)
  noData: {
    noData: "NO DATA",
    stale: "STALE",
    unusable: "SIGNAL UNUSABLE",
  },
  noDataLabel: (status: string) => `No value: ${status}`,

  // Alarm bar and escalation (F-ALM-04/05)
  alarmBarLabel: "Active alarms",
  noActiveAlarms: "No active alarms",
  moreAlarms: (n: number) => `+${String(n)} more`,
  alarmState: {
    confirming: "Confirming: re-measure now",
    active: "Active",
    cleared_awaiting_ack: "Condition cleared: acknowledge",
  },
  acknowledge: "Acknowledge",
  silence: "Silence",
  silenced: "Silenced",
  escalate: "Escalate",
  escalationLabel: "Escalation",
  audience: {
    astronaut: "Astronaut",
    cmo: "Medical officer",
    sound: "All stations sound",
    crew: "Whole crew",
    ground: "Ground",
    reminder: "Reminder",
  },
  stageDelivered: "Delivered",
  stagePending: "Pending",

  // Empty states
  emptyTitle: "Nothing here yet",
} as const;

export type Messages = typeof en;

/** The active catalog (English in v1.0). */
export const strings: Messages = en;
