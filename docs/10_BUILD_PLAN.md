# 10. Build plan for Claude Code

Work one milestone at a time. Start each Claude Code session with:
> Read CLAUDE.md, docs/01_FEATURE_LOCK.md and milestone Mx in docs/10_BUILD_PLAN.md. Plan the work
> as small steps, show me the plan, then implement step by step with tests. Stop at the gate.

Each gate: all tests, clippy and UI checks pass; the listed acceptance checks are demonstrated;
`docs/trace.csv` updated. Commit per step with requirement IDs.

---

## M0. Repository and tooling (F-SYS-06, R-OPS-03)
**Prompt:** "Create the Cargo workspace and folders exactly as in docs/03_ARCHITECTURE.md §2, a
Tauri 2 app in src-tauri with a React 19 + Vite + Tailwind v4 UI in ui/, tauri-specta bindings,
pnpm workspace, CI workflow per docs/09_VV_PLAN.md §4 (including the release-feature guard and a
'no command API' check for caelum-vehicle and caelum-devices), cargo-deny config, and a minimal
window that shows the app version. Copy the prototype's tokens.css and Outfit font (local)."
**Gate:** `pnpm tauri build` produces an installer offline; CI green; empty crates compile.

## M1. Domain, storage, security core (F-SYS-01/02/03/07, R-SEC-01..03)
**Prompt:** "Implement caelum-domain (parameter catalog per docs/05_DATA_MODEL.md §1, units with
conversions, TAI/UTC/MET time types, typed IDs), caelum-store (SQLCipher via rusqlite with key
from the OS keystore, refinery migrations for all tables in §2), and caelum-security (users,
roles, argon2, sessions with auto-lock, re-auth, break-glass, Ed25519 instance key, signed
hash-chained audit log with verify). Expose login/logout/me and audit commands via tauri-specta."
**Gate:** DB file unreadable without key; audit tamper (any field incl. time) detected; role
matrix tests for every command; property tests for unit conversions.

## M2. Ingest pipeline and device framework (F-ACQ-01/02/04/09/10/11, R-CLN-02)
**Prompt:** "Implement caelum-ingest (validation, unit conversion, plausibility, provenance, device
clock correction, rejection reasons) and caelum-devices: the Adapter trait, device registry with
assignment, BLE central with btleplug (scan, pair, bond, auto-reconnect, notifications), decoders
for HRS, PLXS spot/continuous, BPS, HTS, WSS, GLS (port and extend the prototype decoders and
tests), USB/serial framework. Add the corresponding emulators to caelum-test-equipment."
**Gate:** emulated devices stream through ingest into the DB; unplug/replug recovers in ≤ 5 s;
no default values anywhere (test that missing fields produce no observation).

## M3. Alarm engine (F-ALM-01/02/03/08, SR-001..006, SR-009/010)
**Prompt:** "Port the prototype alert engine and its tests to caelum-alarms in Rust: ruleset in
signed TOML (rules from docs/06 §3), signature + approver verification at load with safe fallback,
per-crew/per-rule state, persistence, hysteresis, data-gap restart, supersession, baseline notices
(median/MAD, observe only), and a structured 'why'. Engine runs synchronously in the ingest path.
Build a tools/ruleset-signer CLI."
**Gate:** 100 % branch coverage of rule evaluation; every SR-001..006 has named tests; a tampered
ruleset is rejected and a technical alarm raised.

## M4. Escalation, technical alarms, notifications (F-ALM-04/05/06/07, SR-007/008/012/014)
**Prompt:** "Implement escalation timers (persisted, restart-safe), acknowledge/silence/resolve
with limits and audit, technical alarms (sensor off, low battery, device lost, stale data, clock
drift, storage low, ruleset invalid), and the event bus that delivers alarm events to the UI.
Implement alarm sounds per docs/06 §2 using bundled audio assets."
**Gate:** escalation timeline test matches docs/06 exactly; restart mid-escalation resumes at the
correct stage; silence expires and re-sounds.

## M5. UI shell and design system (07_UI_SPEC §1–3, F-SYS-10)
**Prompt:** "Build the desktop layout from docs/07 §3 (alarm bar, nav rail, crew strip, content,
status footer) and port the prototype primitives listed in §2 into ui/src/components with
Storybook-style test pages. Add NoData, AlarmBar, EscalationTracker, LevelGlyph incl. technical
square. Implement theme, text size up to 200 %, glove mode, reduced motion, high contrast, keyboard
navigation and role-based navigation. Login screen and break-glass emergency entry."
**Gate:** axe accessibility checks pass; keyboard-only walkthrough; 200 % text and glove mode
screenshots reviewed; no screen shows a number without a data source.

## M6. Core crew screens (Home, Body, Alerts, Devices, Settings)
**Prompt:** "Implement Home, Body, Alerts, Devices and Settings per docs/07 §4 on real data via
typed IPC and live events: live vitals with status, live ECG strip from waveform chunks, trends
with baseline band, alarm list with why drawer, escalation tracker, acknowledge/silence, device
pairing and assignment flow."
**Gate:** end-to-end test with emulated devices: pairing → live values → induced desaturation →
alarm → escalation visible → acknowledge stops escalation.

## M7. Signal processing and sensor kit (F-SIG-01..07, F-ACQ-03/05)
**Prompt:** "Implement caelum-signal (ECG filtering + QRS with tests at 40–180 bpm with noise and
baseline wander, HRV, SQIs, respiration from RIP and EDR, SpO₂ with motion rejection, core
temperature Kalman model, activity, PAT and BP trend) and the GHS (IEEE 11073-10206) decoder and
the CAELUM sensor-kit BLE protocol per its ICD. Derived observations carry algorithm versions."
**Gate:** accuracy targets on PhysioNet reference records in CI; GHS ECG stream renders in the UI.

## M8. Clinical files and medical screens (F-ACQ-06/07/08, F-CLN-01..07)
**Prompt:** "Implement DICOM import (file drop, folder watch, C-STORE SCP) with dicom-rs, ECG
import (EDF/EDF+, HL7 aECG, DICOM waveform, SCP-ECG), FHIR R4 import, the encrypted attachment
store, and the Medical and Checks screens: crew overview, encounters with sign-off, ECG viewer
with calipers, DICOM viewer, protocol checklists linked to alarms, scheduled checks including the
PVT reaction test, and the private daily check-in."
**Gate:** reference DICOM and ECG files open and measure correctly; checklist run is audited and
attached to its alarm.

## M9. Vehicle bridge, environment, radiation (F-ENV-01..04, F-GND-11, R-INT-04)
**Prompt:** "Implement caelum-vehicle: cFS message decoding from SBN/UDP for configured MIDs
(environment, radiation, exercise, time), vehicle time correlation, and the health-status
telemetry publisher restricted to an allow-listed MID range. Build the Environment screen with
radiation dose and correlation view. Add a cFS packet source to test equipment."
**Gate:** read-only check passes in CI; environment alarms (CO₂, O₂) fire from vehicle data.

## M10. Ground integration and medical data system (F-GND-01..09, F-GND-12..14, SR-015/016)
**Prompt:** "Implement caelum-ground: CCSDS Space Packets with CUC time via spacepackets, APID plan
from signed config, XTCE generator from the parameter catalog, Yamcs UDP/TCP link, store-and-forward
queue with channels and allow-lists, Ed25519 signing, CFDP class 1/2 via cfdp-rs for event packets
and files, signed uplink verification and application (ruleset, protocols, models, config), FHIR
R4 export. Implement caelum-mds: FHIR R4 client to the onboard medical data system with outbox /
inbox, idempotent sync, read-back with de-duplication, health monitoring, and routing modes A / A' / B
per docs/03 §1 and docs/04 §3a. Implement the `ground` deployment profile (flight surgeon / BME
console) and the plugins/openmct package (telemetry provider, crew board, alert timeline)."
**Gate:** a real Yamcs container decodes CAELUM packets using the generated XTCE; Open MCT shows
crew status; CFDP transfer completes across a simulated 30-day LOS; ops channel contains no
physiology; with a FHIR server (HAPI FHIR container) standing in for the medical data system,
records sync both ways; **unplug the medical system mid-emergency: crew and ground (path B) still
get the alarm, nothing is lost, and the record syncs on reconnect** (SR-015, R-INT-06/07).

## M11. Fail-over and crew stations (F-SYS-04/05, R-OPS-02)
**Prompt:** "Implement caelum-cluster: mutual-TLS crew LAN service (axum), secondary stations,
primary→standby signed event-log replication, heartbeat, promotion with epoch fencing, device
reconnection after takeover, and the instance role in the status footer."
**Gate:** kill primary under full load: standby takes over ≤ 10 s, no acknowledged data lost,
escalations continue at the right stage.

## M12. Models (F-ML-01..04)
**Prompt:** "Implement caelum-ml with ort: package verification, registry, feature builders, time
limits, explanations, validation status in UI. Integrate M1, M2, M4 and M9 packages produced by
the ml/ pipeline (docs/08) as advisory notices; they must never change alarm rules (test this)."
**Gate:** SR-002 test with adversarial model outputs; models meet their gates on frozen sets.

## M13. Hardening and release readiness
**Prompt:** "Run the performance, soak and fault-injection campaigns in docs/09; fix all findings.
Produce SBOM, threat model, hazard log update, user manual, ICDs for every interface, and signed
installers. Prepare the flight-surgeon review package for the ruleset and protocols."
**Gate:** all budgets met; 14-day soak clean; usability summative evaluation passed; review package complete.
