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
  unknownError: "unknown error",

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
  screenNoData: "No data for this screen on this station yet.",

  // Shell (docs/07 §3)
  skipToContent: "Skip to content",
  navLabel: "Screens",
  screen: {
    home: "Home",
    body: "Body",
    alerts: "Alerts",
    checks: "Checks",
    env: "Environment",
    medical: "Medical",
    devices: "Devices",
    ground: "Ground",
    settings: "Settings",
  },
  crewStripLabel: "Crew member",
  crewNotSelected: "No crew member selected",
  footerLabel: "Station status",
  linkState: "Device links",
  groundLink: "Ground link",
  instanceRole: "Instance",
  signedInAs: (name: string, role: string) => `${name} (${role})`,
  role: {
    astronaut: "Astronaut",
    cmo: "Crew medical officer",
    flight_surgeon: "Flight surgeon",
    bme: "Biomedical engineer",
    mcc: "Mission control",
    administrator: "Administrator",
    device_gateway: "Device gateway",
  },
  signOut: "Sign out",

  // Sign-in, lock, first administrator, break-glass (F-SYS-01, CR-007, SR-014)
  signInTitle: "Sign in",
  username: "Sign-in name",
  secret: "PIN or password",
  signIn: "Sign in",
  signingIn: "Signing in…",
  setupTitle: "Create the administrator account",
  setupHelp: "This station has no accounts yet. Create the administrator; crew accounts are added afterwards.",
  setupCreate: "Create administrator",
  lockedTitle: "Session locked",
  lockedHelp: (name: string) => `Signed in as ${name}. Enter your PIN or password to unlock. Alarms stay visible and audible.`,
  unlock: "Unlock",
  emergency: "Emergency",
  emergencyHelp: "Open the emergency view without signing in",
  emergencyTitle: "Emergency view",
  emergencyAlarms: "Active alarms, whole crew",
  emergencyNoAlarms: "No active alarms on this station.",
  emergencyNotAudited: "This opening could not be recorded in the audit log.",
  back: "Back",
  errors: {
    signInRequired: "Sign in first.",
    locked: "The session is locked.",
    notPermitted: "Not available for your role.",
    invalidCredentials: "Sign-in name or PIN/password is not correct.",
    throttled: (s: number) => `Too many attempts. Try again in ${String(s)} s.`,
    accountDisabled: "This account is disabled.",
    notInteractive: "This account cannot sign in here.",
    reauthRequired: "Confirm with your PIN or password first.",
    unavailable: "The station's records are unavailable.",
    failed: "The action failed.",
    rejected: (reason: string) => `Refused: ${reason}.`,
  },

  // Administration (F-SYS-07, CR-012 §2–3, CR-007 §4)
  adminTitle: "Administration",
  rosterTitle: "Crew roster",
  rosterHelp: "Apply a signed crew roster: the roster file and its signature file (.sig.toml).",
  rosterFile: "Roster file",
  rosterSignature: "Signature file",
  rosterApply: "Apply roster",
  rosterApplied: (version: string, n: number) => `Roster ${version} applied: ${String(n)} crew members on the roster.`,
  rosterDevelopment: "Signed with development keys: not for operational use.",
  rosterEmpty: "No crew roster has been applied on this station.",
  rosterChooseFiles: "Choose the roster file and its signature file.",
  rosterReadFailed: "The files could not be read.",
  crewColumns: { name: "Name", position: "Position", mission: "Mission", status: "Status" },
  crewActive: "On the roster",
  crewInactive: "No longer on the roster",
  usersTitle: "Accounts",
  userColumns: { username: "Sign-in name", role: "Role", crew: "Crew member", status: "Status", action: "Action" },
  userActive: "Active",
  userDisabled: "Disabled",
  userDisable: "Disable",
  userDisableLabel: (name: string) => `Disable account ${name}`,
  userDisabledDone: (name: string) => `Account ${name} disabled.`,
  userNoCrew: "—",
  newUserTitle: "New account",
  newUserRole: "Role",
  newUserCrew: "Crew member",
  newUserCrewChoose: "Choose a crew member",
  newUserSecret: "Initial PIN or password",
  newUserCreate: "Create account",
  userCreated: (name: string) => `Account ${name} created.`,
  reauthTitle: "Confirm it is you",
  reauthHelp: "This change needs your PIN or password, entered within the last 2 minutes.",
  reauthConfirm: "Confirm and continue",
  reauthCancel: "Cancel",

  // Settings (F-SYS-10)
  settingsTitle: "Display",
  theme: "Theme",
  themes: { light: "Light", dark: "Dark", "high-contrast": "High contrast" },
  textSize: "Text size",
  textScale: (pct: number) => `${String(pct)} %`,
  gloveMode: "Glove mode (larger controls)",
  reducedMotion: "Reduce motion",
} as const;

export type Messages = typeof en;

/** The active catalog (English in v1.0). */
export const strings: Messages = en;
