/**
 * User-visible strings (English). Kept in one place so they move into the localisation framework
 * without hunting through components (F-SYS-11, M5).
 */
export const strings = {
  productName: "CAELUM",
  tagline: "Onboard crew health monitoring",
  loading: "Starting…",
  version: (version: string) => `Version ${version}`,
  versionUnavailable: "Version unavailable",
  coreNoResponse: (reason: string) => `The application core did not respond: ${reason}`,
  rulesetLabel: "Alarm ruleset",
  developmentRuleset: (version: string) => `DEVELOPMENT RULESET ${version}: not for operational use`,
  rulesetFallback: (version: string) => `The installed alarm ruleset was rejected; ruleset ${version} is in use`,
  rulesetSafeState: "Physiological alarm rules are not running: no verified ruleset",
  rulesetUnknown: "Alarm ruleset status unavailable",
} as const;
