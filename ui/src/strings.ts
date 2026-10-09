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
} as const;
