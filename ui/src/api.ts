import type {
  AcknowledgeScope,
  AppInfo,
  BreakGlass,
  CommandError,
  CrewMember,
  RoleName,
  RosterResult,
  SessionInfo,
  UserItem,
} from "./bindings";
import type { RulesetResult } from "./RulesetBanner";

/** A command result as the generated bindings return it. */
export type Res<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };

/** The IPC commands the window uses (the generated bindings by default; injectable for tests). */
export interface Api {
  readonly appInfo: () => Promise<AppInfo>;
  readonly rulesetStatus: () => Promise<RulesetResult>;
  readonly setupStatus: () => Promise<Res<boolean>>;
  readonly setupCreateAdministrator: (username: string, secret: string) => Promise<Res<null>>;
  readonly authMe: () => Promise<Res<SessionInfo>>;
  readonly authLogin: (username: string, secret: string) => Promise<Res<SessionInfo>>;
  readonly authLogout: () => Promise<Res<null>>;
  readonly authReauth: (secret: string) => Promise<Res<null>>;
  readonly breakGlassOpen: () => Promise<Res<BreakGlass>>;
  readonly alarmAcknowledge: (alarm: string) => Promise<Res<AcknowledgeScope>>;
  readonly alarmSilence: (alarm: string, seconds: number) => Promise<Res<number>>;
  // Crew roster and accounts (F-SYS-07, CR-012 §2–3)
  readonly crewList: () => Promise<Res<CrewMember[]>>;
  readonly configApplyCrew: (bundle: string, signature: string) => Promise<Res<RosterResult>>;
  readonly userList: () => Promise<Res<UserItem[]>>;
  readonly userCreate: (username: string, role: RoleName, crewMemberId: string | null, secret: string) => Promise<Res<string>>;
  readonly userDisable: (userId: string) => Promise<Res<null>>;
}
