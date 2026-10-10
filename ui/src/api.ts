import type {
  AcknowledgeScope,
  AlarmEntry,
  AlarmInfo,
  AlarmReportData,
  Attachment,
  Checkin,
  CheckinEntry,
  ChecklistRun,
  Encounter,
  GroundStatus,
  ModelItem,
  MedicationEntry,
  OverviewEntry,
  ProtocolItem,
  Pvt,
  Step,
  AppInfo,
  AuditItem,
  AuditStatus,
  BreakGlass,
  CommandError,
  CrewMember,
  DeviceEntry,
  NearbyDevice,
  PairResult,
  RoleName,
  RosterResult,
  SessionInfo,
  TrendData,
  UserItem,
  VitalItem,
  WaveformChunk,
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
  // Crew screens (CR-012 §4)
  readonly vitalsLatest: (crewMemberId: string) => Promise<Res<VitalItem[]>>;
  readonly vitalsTrend: (crewMemberId: string, parameter: string, windowS: number) => Promise<Res<TrendData>>;
  readonly waveformRecent: (crewMemberId: string, signal: string, seconds: number) => Promise<Res<WaveformChunk[]>>;
  readonly alarmList: () => Promise<Res<AlarmEntry[]>>;
  readonly alarmDetail: (alarm: string) => Promise<Res<AlarmInfo>>;
  readonly alarmReport: (windowS: number) => Promise<Res<AlarmReportData>>;
  readonly alarmEscalate: (alarm: string) => Promise<Res<null>>;
  readonly deviceList: () => Promise<Res<DeviceEntry[]>>;
  readonly deviceAssign: (deviceId: string, crewMemberId: string, deviceUser: number | null) => Promise<Res<null>>;
  readonly deviceUnassign: (deviceId: string) => Promise<Res<null>>;
  readonly deviceScan: () => Promise<Res<NearbyDevice[]>>;
  readonly devicePair: (address: string, name: string | null, services: number[]) => Promise<Res<PairResult>>;
  readonly auditList: () => Promise<Res<AuditItem[]>>;
  readonly auditVerify: () => Promise<Res<AuditStatus>>;
  // Clinical workflows (CR-019)
  readonly crewOverview: () => Promise<Res<OverviewEntry[]>>;
  readonly encounterList: (crewMemberId: string) => Promise<Res<Encounter[]>>;
  readonly encounterSave: (
    id: string | null,
    crewMemberId: string,
    subjective: string | null,
    objective: string | null,
    assessment: string | null,
    plan: string | null,
    attachments: string[],
  ) => Promise<Res<string>>;
  readonly encounterSign: (id: string) => Promise<Res<null>>;
  readonly checkinSubmit: (answers: Checkin, tier: string) => Promise<Res<null>>;
  readonly checkinList: (crewMemberId: string) => Promise<Res<CheckinEntry[]>>;
  readonly pvtSubmit: (reactionMs: number[]) => Promise<Res<Pvt>>;
  readonly medicationLog: () => Promise<Res<MedicationEntry[]>>;
  readonly medicationAdd: (
    crewMemberId: string | null,
    kind: string,
    item: string,
    amount: number | null,
    unit: string | null,
    note: string | null,
  ) => Promise<Res<string>>;
  readonly protocolList: () => Promise<Res<ProtocolItem[]>>;
  readonly checklistStart: (protocolId: string, crewMemberId: string | null, alarm: string | null) => Promise<Res<string>>;
  readonly checklistUpdate: (run: string, step: number, mark: Step, complete: boolean) => Promise<Res<null>>;
  readonly checklistList: () => Promise<Res<ChecklistRun[]>>;
  readonly attachmentImport: (crewMemberId: string, kind: string, filename: string, bytes: number[]) => Promise<Res<string>>;
  readonly attachmentList: (crewMemberId: string | null) => Promise<Res<Attachment[]>>;
  // Ground, path B and models (M9, M10, M12)
  readonly groundStatus: () => Promise<Res<GroundStatus>>;
  readonly modelList: () => Promise<Res<ModelItem[]>>;
}

/** Subscriptions to the value-free change events (new readings, device changes). */
export interface DataEvents {
  readonly onObservations: (handler: (crewId: string) => void) => Promise<() => void>;
  readonly onDevices: (handler: () => void) => Promise<() => void>;
}
