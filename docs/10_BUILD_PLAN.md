# 10. Build plan for Claude Code

Work one milestone at a time. Start each Claude Code session with:
> Read CLAUDE.md, docs/01_FEATURE_LOCK.md and milestone Mx in docs/10_BUILD_PLAN.md. Plan the work
> as small steps, show me the plan, then implement step by step with tests. Stop at the gate.

Each gate: all tests, clippy and UI checks pass; the listed acceptance checks are demonstrated;
`docs/trace.csv` updated. Commit per step with requirement IDs. When a gate passes, its row below
is set to `complete` with the date and commit. The trace check enforces tests only for milestones
marked `complete` (`09` §3).

## Milestone status
| Milestone | Status | Date | Commit |
|---|---|---|---|
| M0 | not started | | |
| M1 | not started | | |
| M2 | not started | | |
| M3 | not started | | |
| M4 | not started | | |
| M5 | not started | | |
| M6 | not started | | |
| M7 | not started | | |
| M8 | not started | | |
| M9 | not started | | |
| M10 | not started | | |
| M11 | not started | | |
| M12 | not started | | |
| M13 | not started | | |

Items marked ⚕ in CR-002/CR-003 are DRAFT clinical content approved by the provisional medical
lead; rulesets stay development-signed (with the DEVELOPMENT RULESET banner) until a licensed
clinician advisor countersigns.

Versioning: the workspace starts at 0.1.0. Version 1.0.0 means every M-priority feature of the
feature lock is complete and gated.

---

## M0. Repository and tooling (F-SYS-06, R-OPS-03, R-SEC-04, R-SEC-07, SR-011, F-SYS-10 tokens)
**Prompt:** "Create the Cargo workspace and folders exactly as in docs/03_ARCHITECTURE.md §2, a
Tauri 2 app in src-tauri with a React 19 + Vite + Tailwind v4 UI in ui/, tauri-specta bindings
(exact release-candidate pin behind a thin wrapper, recorded in docs/third-party-risk.md), pnpm
workspace, CI workflow per docs/09_VV_PLAN.md §4 (including the release-feature guard, the
write-exception guard for caelum-vehicle and caelum-devices against an empty
docs/write-exceptions.md, and the trace script with a milestone column), cargo-deny config, the
offline tool cache and its scripts per docs/OFFLINE_BUILD.md, and a minimal window that shows the
app version. Bring over the prototype's tokens.css with the accessibility fixes in docs/07 §1
(alarm tokens as DRAFT), the Outfit font via @fontsource, and the automated contrast test. Add
LICENSE (Apache-2.0) and NOTICE attributions for bundled third-party assets."
**Gate:** a release installer is built from the checksummed tool cache with the network disabled
and installs with no network; CI green; empty crates compile; every guard fails on a planted
violation; the contrast test passes for the light, dark and high-contrast themes.

## M1. Domain, storage, security core (F-SYS-01/02/03/07, R-SEC-01..03, R-OPS-05)
**Prompt:** "First draft docs/ROLE_MATRIX.md and stop for approval. Principles: astronaut = own
data; CMO = all crew onboard; flight surgeon = all crew, ground; BME = devices, technical alarms,
data quality, clinical values read-only; mission control = status only; administrator =
configuration, no clinical content (separation of duties); device gateway = ingest only. Check-in
tiers: Private (astronaut only), Medical (CMO + flight surgeon), Status (operational level only).
After approval, implement caelum-domain (parameter catalog per docs/05_DATA_MODEL.md §1 including
stale_after_s and the CR-002 parameters, every LOINC code verified against the LOINC table in
tools/offline/loinc/ (git-ignored; commit only the subset of codes used, with the LOINC copyright
notice and licence attribution), units
with conversions, TAI/UTC/MET time types with the bundled IERS leap-second table and the CCSDS
1958 epoch, typed IDs), caelum-store (SQLCipher via rusqlite with key from the OS keystore,
refinery migrations for all tables in §2), and caelum-security (users, roles, argon2, sessions with
auto-lock, re-auth, break-glass, Ed25519 instance key, signed hash-chained audit log with verify).
Expose login/logout/me and audit commands via tauri-specta."
**Gate:** ROLE_MATRIX.md approved; DB file unreadable without key; audit tamper (any field incl.
time) detected; role matrix tests for every command against ROLE_MATRIX.md; property tests for unit
conversions; leap-second conversion tests across a leap second.

## M2. Ingest pipeline and device framework (F-ACQ-01/02/04/09/10/11, R-CLN-02, R-SEC-07, SR-017 ⚕)
**Prompt:** "Implement caelum-ingest (validation, unit conversion, plausibility, provenance, device
clock correction, rejection reasons) and caelum-devices: the Adapter trait, device registry with
assignment, BLE central with btleplug (scan, auto-reconnect, notifications), pairing and bonding
platform modules (Windows WinRT DevicePairing via the windows crate, Linux BlueZ via bluer, macOS
system pairing on encrypted access) with the guided OS-pairing fallback, link metrics (rssi,
packet_loss_pct, reconnect_count), decoders for HRS, PLXS spot/continuous, BPS, HTS, WSS, GLS
(port and extend the prototype decoders and tests), USB/serial framework. Every device write gets
a reviewed entry in docs/write-exceptions.md. Reject readings flagged as demonstration or test
data in release builds; thermometer readings go to body_temp_measured with their site code
(unknown if absent). Add the corresponding emulators to caelum-test-equipment."
**Gate:** emulated devices stream through ingest into the DB; unplug/replug recovers in ≤ 5 s;
no default values anywhere (test that missing fields produce no observation); every device write
has a reviewed register entry and the guard passes; demonstration/test-flagged readings rejected;
no thermometer reading reaches core_temp.

## M3. Alarm engine (F-ALM-01/02/03/08, SR-001..006, SR-009/010, SR-018 ⚕)
**Prompt:** "Port the prototype alert engine and its tests to caelum-alarms in Rust: ruleset in
signed TOML (rules, categories and per-rule max_gap_s from docs/06 §3), signature + approver
verification at load with safe fallback (development keys only in development builds with the
DEVELOPMENT RULESET banner; release builds accept only flight keys with a medical approver
signature), per-crew/per-rule state, persistence, hysteresis, data-gap restart, supersession,
threshold and absence-of-event rule types (absence tested with synthetic event streams), the spot
evaluation mode per SR-019 (CONFIRMING, confirm_window_s, recheck_timeout_s, not-confirmed) with
n-of-m spot baselines, the measured-temperature rules with per-site thresholds and the THERMAL
supersession group, unknown context handled per SR-018, baseline notices (median/MAD, observe
only, parameters in docs/06 §3), the AF rule as an observe notice, and a structured 'why'. Engine runs synchronously in the ingest
path. Build a tools/ruleset-signer CLI."
**Gate:** caelum-alarms ≥ 95 % region coverage; exhaustive table-driven and property tests for
every rule; table-driven tests for every spot-mode branch; every SR-001..006, SR-018 and SR-019
has named tests; a tampered ruleset is rejected and a
technical alarm raised; a release build rejects a development-signed ruleset; nightly
branch-coverage report attached; independent review of caelum-alarms documented.

## M4. Escalation, technical alarms, notifications, self-test (F-ALM-04/05/06/07, F-SYS-09, SR-007/008/012/014/015 ⚕)
**Prompt:** "Implement escalation timers (persisted, restart-safe), acknowledge/silence/resolve
with limits and audit, technical alarms (sensor off, low battery, device lost, stale data per
stale_after_s, clock drift, storage low, ruleset invalid), the start-up self-test and continuous
watchdogs (storage, clock, tasks, devices), and the event bus that delivers alarm events to the
UI. Crew stations always sound; the wristband alert through the Immediate Alert Service (register
entry, category c) is in addition. The ground stage emits an OPS escalation event with the field
set in docs/06 §3a (its path B transport comes in M10). Implement alarm sounds per docs/06 §2 using
bundled audio assets."
**Gate:** escalation timeline test matches docs/06 exactly; restart mid-escalation resumes at the
correct stage; silence expires and re-sounds; with no wristband connected the crew stations still
sound; a failed self-test raises a technical alarm.

## M5. UI shell and design system (07_UI_SPEC §1–3, F-SYS-10, F-SYS-11)
**Prompt:** "Build the desktop layout from docs/07 §3 (alarm bar, nav rail, crew strip, content,
status footer) and port the prototype primitives listed in §2 into ui/src/components with
Storybook-style test pages. Add NoData, AlarmBar, EscalationTracker, LevelGlyph incl. technical
square. Implement theme (light, dark, high contrast), text size up to 200 %, glove mode, reduced
motion, keyboard navigation and role-based navigation, and the localisation framework (English
strings). Login screen and break-glass emergency entry."
**Gate:** axe accessibility checks pass (Vitest browser mode); contrast test passes; keyboard-only
walkthrough; 200 % text and glove mode screenshots reviewed; no screen shows a number without a
data source; no user-visible string outside the localisation catalog.

## M6. Core crew screens (Home, Body, Alerts, Devices, Settings, Audit; F-ALM-09)
**Prompt:** "Implement Home, Body, Alerts, Devices, Settings and Audit per docs/07 §4 on real data
via typed IPC and live events: live vitals with status, live ECG strip from waveform chunks, trends
with baseline band, alarm list with why drawer, escalation tracker, acknowledge/silence, device
pairing and assignment flow, the signed audit log with verification status, and alarm history,
statistics and the alarm-fatigue report (F-ALM-09)."
**Gate:** end-to-end test (tauri-driver, Windows and Linux) with emulated devices: pairing → live
values → induced desaturation → alarm → escalation visible → acknowledge stops escalation; audit
tamper shown as broken in the Audit screen.

## M7. Signal processing and sensor kit (F-SIG-01..07, F-ACQ-03/05)
**Prompt:** "Implement caelum-signal (ECG filtering + QRS with tests at 40–180 bpm with noise and
baseline wander, HRV, SQIs, respiration from RIP and EDR, SpO₂ with motion rejection, core
temperature Kalman model, activity, PAT and BP trend) and the GHS (IEEE 11073-10206) decoder and
the CAELUM sensor-kit BLE protocol per its ICD. Derived observations carry algorithm versions.
Wire asystole.critical to real QRS detection."
**Gate:** accuracy targets on PhysioNet reference records in CI; GHS ECG stream renders in the UI;
asystole fires on a reference record with a pause and not on noise with poor SQI.

## M8. Clinical files, medical screens, retention and export (F-ACQ-06/07/08, F-CLN-01..08, F-SYS-08 part)
**Prompt:** "Implement DICOM import (file drop, folder watch, C-STORE SCP) with dicom-rs, ECG
import (EDF/EDF+, HL7 aECG, DICOM waveform, SCP-ECG), FHIR R4 import (Observations without a unit
rejected; LOINC 2708-6 not mapped to SpO₂), the encrypted attachment store, retention policies,
CSV and EDF+ export, and the Medical and Checks screens: crew overview, encounters with sign-off,
ECG viewer with calipers, DICOM viewer, protocol checklists linked to alarms, scheduled checks
including the PVT reaction test, the private daily check-in, and the medication and medical-kit
log (F-CLN-08)."
**Gate:** reference DICOM and ECG files open and measure correctly; checklist run is audited and
attached to its alarm; retention removes only eligible data; CSV and EDF+ exports reopen in a
reference reader.

## M9. Vehicle bridge, environment, radiation (F-ENV-01..04, F-GND-11 subscribe side, R-INT-04)
**Prompt:** "Implement caelum-vehicle, subscribe only with no write API: cFS message decoding from
SBN/UDP for configured MIDs (environment, radiation including spe_state, exercise, time) and
vehicle time correlation (MET epoch from signed mission configuration). Complete the inbound
sections of docs/icd/ICD-VEH.md. Build the Environment screen with radiation dose and correlation
view. Add a cFS packet source to test equipment."
**Gate:** write-exception guard passes in CI; environment alarms (CO₂, O₂) fire from vehicle data.

## M10. Ground integration and medical data system (F-GND-01..09, F-GND-11 publish side, F-GND-12..14, F-SYS-08 part, SR-015/016)
**Prompt:** "Implement caelum-ground: CCSDS Space Packets with CUC time (1958 TAI epoch) via
spacepackets, APID plan from signed config, XTCE generator from the parameter catalog, Yamcs
UDP/TCP link, store-and-forward queue with channels and allow-lists, Ed25519 signing, the path B
status publisher (telemetry-only packet type; to a configurable UDP endpoint until ICD-VEH OI-1 is
agreed) carrying the OPS field set in docs/06 §3a including ground escalation, CFDP class 1/2 via
cfdp-rs for event packets and files with transactions only during contact windows and the queue
suspending and resuming them (build any missing pieces on top of cfdp-rs), signed uplink
verification and application (ruleset, protocols, models, config), FHIR R4 export, and the
de-identified research export. Implement caelum-mds: FHIR R4 client to the onboard medical data
system with outbox / inbox, idempotent sync, read-back with de-duplication, health monitoring, and
routing modes A / A' / B per docs/03 §1 and docs/04 §3a. Implement the `ground` deployment profile
(flight surgeon / BME console) and the plugins/openmct package (telemetry provider, crew board,
alert timeline)."
**Gate:** a real Yamcs container decodes CAELUM packets using the generated XTCE; Open MCT shows
crew status; over a simulated 30-day LOS the queue holds every item, CFDP transactions are
suspended at LOS and resumed at the next contact window, and every file completes with no loss;
the ops channel contains no physiology (no measured value, parameter name, rule id or free text);
the publisher cannot build a telecommand packet; with a FHIR server (HAPI FHIR container) standing
in for the medical data system, records sync both ways; **unplug the medical system
mid-emergency: crew and ground (path B) still get the alarm, nothing is lost, and the record syncs
on reconnect** (SR-015, R-INT-06/07).

## M11. Fail-over, crew stations, kiosk and multi-window (F-SYS-04/05, F-SYS-13, R-OPS-02)
**Prompt:** "Implement caelum-cluster: mutual-TLS crew LAN service (axum), secondary stations,
primary→standby signed event-log replication, heartbeat, promotion with epoch fencing, device
reconnection after takeover, and the instance role in the status footer. Add kiosk mode for a
wall-mounted crew display and multiple windows (F-SYS-13)."
**Gate:** kill primary under full load: standby takes over ≤ 10 s, no acknowledged data lost,
escalations continue at the right stage; a kiosk station shows the crew-wide emergency view.

## M12. Models (F-ML-01..04)
**Prompt:** "Implement caelum-ml with ort (load-dynamic, pinned ONNX Runtime binary from the tool
cache): package verification, registry, feature builders, time limits, explanations, validation
status in UI. Integrate M1, M2, M4 and M9 packages produced by the ml/ pipeline (docs/08) as
advisory notices; they must never change alarm rules (test this). AF stays an observe notice until
M2 is VALIDATED; promoting it to warning needs a CR."
**Gate:** SR-002 test with adversarial model outputs; models meet their gates on frozen sets.

## M13. Should-features, hardening and release readiness (F-ACQ-12/13, F-SYS-12)
**Prompt:** "Implement the adapter plug-in SDK (F-ACQ-12), smart-garment vendor adapters via the
SDK (F-ACQ-13) and resource budget mode (F-SYS-12). Run the performance, soak and fault-injection
campaigns in docs/09; fix all findings. Produce SBOM, threat model, hazard log update, user manual,
ICDs for every interface, and signed installers (offlineInstaller; fixedRuntime for flight builds).
Prepare the flight-surgeon review package for the ruleset and protocols."
**Gate:** all budgets met; 14-day soak clean; usability summative evaluation passed; review package complete.
