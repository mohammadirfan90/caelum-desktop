# CAELUM Desktop: progress and context

Living record of where the project stands, why things are the way they are, what is open, and how
to continue. Updated with every push (the commit history holds the details; this file holds the
picture). Last updated: 2026-10-10 (M5 gate evidence written: axe and keyboard walkthrough in a real browser).

## 1. Milestones
| Milestone | State | Evidence | Open |
|---|---|---|---|
| M0 Repository and tooling | **complete** (13292b4) | `docs/gates/M0.md`, CI run 38023371185 | Physical network-off install test moved to M13 (CR-005) |
| M1 Domain, storage, security core | **complete** (13292b4) | `docs/gates/M1.md` | First-administrator enrolment arrives with the M3/M6 login screen (CR-007 §4) |
| M2 Ingest pipeline and device framework | **complete** (8550faf) | `docs/gates/M2.md` | Physical devices on the hardware-in-the-loop bench (docs/09) |
| M3 Alarm engine | **complete** (8550faf) | `docs/gates/M3.md` (independent review: Mohammad Irfan, 2026-10-10) | ⚕ clinical content stays DRAFT |
| M4 Escalation, technical alarms, notifications, self-test | **complete** (7c782a7, CI run 38036944297) | `docs/gates/M4.md` | Wristband against a real device on the HIL bench; device runtime wiring with the Devices screen (M6) |
| M5 UI shell and design system | **complete** (7c782a7, CI run 38036944297) | `docs/gates/M5.md`, screenshots in `docs/gates/M5-screenshots/` | Screenshot review by the milestone reviewer (Mohammad Irfan); screen-reader usability session (docs/09) |
| M6 Core crew screens | **in progress**: CR-012; roster, accounts, read commands, device runtime, crew screens Home/Body/Alerts/Devices/Audit | | End-to-end gate (tauri-driver specs written, not yet run) |
| M7 Signal processing | **in progress**: CR-013, CR-021; library and every algorithm integrated (GHS and sensor kit; QRS → asystole; SQIs; heart rate; respiration from RIP and EDR; PPG pulse rate; activity and sleep/wake; core temperature; baselines) | `docs/gates/M7.md` | Live GHS stream on screen (e2e/HIL); ICD-SENSOR-KIT OI-3 (SpO₂ calibration) and OI-4 (PPG polarity) |
| M8 Clinical files and medical screens | **in progress**: CR-014, CR-019, CR-022; everything built except the C-STORE SCP | `docs/gates/M8.md` | Test phase (retention, reference reader, external reference files); C-STORE SCP needs a network CR; event-packet attachments with M10 CFDP |
| M9 Vehicle bridge | **complete** (local evidence, CR-020) | `docs/gates/M9.md` | `spe_state` values wait for ICD-VEH OI-2 (external) |
| M10 Ground and MDS | **in progress**: CR-015, CR-016, CR-024; ground, path B, MDS, onboard ground downlink (summaries, event packets, contact windows, CFDP class 2) wired | `docs/gates/M10.md` | Ground console profile (F-GND-08); container gate runs (Yamcs, Open MCT, HAPI FHIR); unplug test |
| M11 Fail-over and crew LAN | **in progress**: CR-017; every item built: provisioning, log and fence, row replication (V9), passive standby and promotion, crew-LAN sign-in and streams, secondary and kiosk crew stations, instance role | `docs/integration/M11.md` | Gate run: kill the primary under load (≤ 10 s, no acknowledged loss, escalation continues); attachment files not replicated (metadata only) |
| M12 Advisory models | **in progress**: CR-018; runtime merged, model status in the UI | `docs/integration/M12.md` | Gate run |
| M13 Release | not started | | |

Status of record: the table in `docs/10_BUILD_PLAN.md` (the trace enforces milestones marked
`complete` there).

## 2. What exists
| Area | Where | Summary |
|---|---|---|
| Domain | `crates/caelum-domain` | 44-parameter catalog (single source of truth), units with explicit conversions only, TAI/UTC/MET with the IERS table, typed IDs, thermometer sites |
| Storage | `crates/caelum-store` | SQLCipher database (key from the OS keystore), migrations V1–V3, repositories for users, audit, devices, observations, alarms, config bundles |
| Security | `crates/caelum-security` | Roles and the command access table (tested against `docs/ROLE_MATRIX.md`), argon2id, sessions (auto-lock, re-auth, throttling, break-glass), keystore, Ed25519 instance key, signed hash-chained audit log |
| Ingest | `crates/caelum-ingest` | Validation, unit conversion, plausibility, clock correction, provenance; rejection codes; no defaults ever |
| Devices | `crates/caelum-devices` | SIG decoders (HRS, PLXS, BPS, HTS, WSS, GLS, CTS), registry with assignment and user slots, link supervisor (reconnect ≤ 5 s), BLE central (btleplug), pairing (WinRT, BlueZ, macOS), read-only serial and HID |
| Alarms | `crates/caelum-alarms`, `rules/ruleset.toml` | Signed TOML ruleset (matches docs/06 §3 rule by rule), strict validation, signature policy, deterministic engine (continuous, spot SR-019, absence, baseline, supersession, latching) |
| Core | `crates/caelum-core` | Station (records, keys, session, M1 commands, ruleset at start-up), acquisition path, alarm service with the SR-009 fallback |
| App | `src-tauri`, `ui` | Tauri 2 shell; IPC: app_info, auth_*, break_glass_open, audit_list, audit_verify, ruleset_status, alarm_acknowledge, alarm_silence, alarm_escalate; UI window with version and the DEVELOPMENT RULESET banner |
| Test equipment | `crates/caelum-test-equipment` | SIG device emulators (test builds only) |
| Tools | `tools/ci`, `tools/offline`, `tools/ruleset-signer` | Guards (release features, writes, npm licences), SBOM, trace, coverage gates; offline tool cache; ruleset signer |

## 3. Decisions (change requests)
| CR | Subject |
|---|---|
| CR-001 | Two paths: FHIR to the medical data system (A), CCSDS status to the vehicle (B) |
| CR-002 | M0-blocking decisions: write-exception register, trace, coverage gates, offline cache, tokens, repo, tauri-specta pin |
| CR-003 | Provisional medical lead (Abdur Rahman FahAd; clinical items DRAFT ⚕), identity (`org.chronostrider.caelum`, ChronoStrider, Apache-2.0), spot mode, measured fever |
| CR-004 | WebView2 Fixed Version runtime shipped in Windows installers |
| CR-005 | Network-trap evidence for offline builds; physical network-off run in M13 |
| CR-006 | Security and storage dependencies; argon2id, audit chain, key handling parameters |
| CR-007 | Sign-in policy (PIN/password, throttling, locked sessions) and user provisioning |
| CR-008 | Quality and flags from Bluetooth SIG profiles (⚕ glucose out-of-range pending) |
| CR-009 | Ruleset format, signing, dependencies; ⚕ draft values and the SR-019 interpretation |
| CR-010 | Escalation audiences, sound and silence semantics (⚕) |
| CR-011 | Technical-alarm limits and the self-test (engineering values) |

Write exceptions: `docs/write-exceptions.md` (WX-001 CCCD enable and WX-002 wristband alert, both
reviewed by Mohammad Irfan, product owner, 2026-10-10). Third-party risks: `docs/third-party-risk.md`.

## 4. Waiting on people
- **Medical lead** on ⚕ DRAFT items: CR-010 escalation audiences and behaviour, CR-003 site classes, CR-008 §3 glucose out-of-range, CR-009 §3
  (ECG SQI gate 0.7, spot interpretation, weight notice category), all docs/06 §3 thresholds.
- **Licensed clinician advisor**: countersignature; until then no flight ruleset key exists and
  release builds start in the SR-009 safe state (no physiological rules) by design.
- **Write-exception reviews**: every new device write needs a reviewer other than its author.

## 5. How to continue
- Read `CLAUDE.md` (rules and commands), `docs/01_FEATURE_LOCK.md`, `docs/06_SAFETY_AND_ALARMS.md`
  and the milestone in `docs/10_BUILD_PLAN.md`.
- Windows builds need Strawberry Perl and NASM on PATH (vendored OpenSSL for SQLCipher).
- After changing the ruleset: bump `[ruleset].version`, re-sign with
  `cargo run -p caelum-ruleset-signer -- sign ... --role author`; the development key lives outside
  the repository (`G:\SpaceApps\caelum-dev-keys`); only its public key is committed.
- Before each push: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo nextest run --workspace`, the test-equipment run, `pnpm -C ui typecheck lint test`,
  `pnpm -C tools/ci check` and `test`, then `pnpm -C tools/ci trace` (CI checks trace.csv is
  current). Update this file.
- Commits name the requirement IDs they implement and carry no tool attribution.

## 6. Log
| Date | Commits | What and why |
|---|---|---|
| 2026-10-09 | M0 steps 1–7 | Workspace, Tauri app, UI tokens, CI, guards, offline tool cache, WebView2 fixed runtime (CR-002..005) |
| 2026-10-10 | 85585b5..13292b4 | M1: domain, SQLCipher store, security core, sign-in and audit IPC with role-matrix tests (CR-006, CR-007) |
| 2026-10-10 | 05f16df..8d670c2 | M2: ingest, decoders, registry, acquisition, link supervision, serial, BLE with WX-001, pairing, clock sync, HID; trace handles OS- and feature-gated tests (CR-008) |
| 2026-10-10 | 5a422b0..4e0a6c4 | M3: signed ruleset and signer, engine, alarm service with SR-009 fallback, coverage gates in CI, DEVELOPMENT RULESET banner (CR-009); M0 and M1 gates closed |
| 2026-10-10 | 5ceaf9d.. | PROGRESS.md added; M3 independent review by Mohammad Irfan recorded; CI concurrency per commit so every pushed commit gets a complete run (pushes happen after every commit) |
| 2026-10-10 | (this push) | rustfmt on the station test (CI run 38030239526 failed `cargo fmt --check` on Ubuntu); the run on this commit is the M2/M3 closing candidate |
| 2026-10-10 | (this push) | M4 step 1: escalation plans moved into the signed ruleset (1.0.0-draft.3, equal to docs/06 by test); sans-I/O escalator with restart resume, SR-008 silence, sound scopes and the docs/06 §3a OPS record (CR-010) |
| 2026-10-10 | (this push) | M4 step 2a: escalator in the alarm service; stages, acknowledgement (onboard and ground stage), silence and escalation stage persisted (migration V4, alarm_notification); open alarms restored into engine and escalator after a restart (no duplicates, sound resumes); alarm authority per ROLE_MATRIX §2 tested; clinical audit actions defined |
| 2026-10-10 | (this push) | M4 step 2b: alarm actions as IPC commands with role and crew-scope checks (astronaut own, CMO all, flight surgeon ground stage only) and clinical audit entries; ROLE_MATRIX §3 rows added |
| 2026-10-10 | (this push) | M4 step 3: technical-alarm monitor (low battery, device lost, stale data, clock drift, storage low) and the start-up self-test (database, audit chain, ruleset, clock and leap table, storage) raising technical alarms (CR-011) |
| 2026-10-10 | (this push) | M4 step 4a: Station::tick runs engine, escalation, technical monitor and watchdogs (clock backwards SR-012, storage every 10 s); observations refresh staleness and battery |
| 2026-10-10 | (this push) | M4 step 4b: alarm event bus to the UI (typed tauri-specta event AlarmBusEvent: alarm state, ended, stage, sound on/off; never values or why); 4 Hz tick thread in the app; alarm actions push their sound changes; restored sounds after a restart |
| 2026-10-10 | (this push) | M4 step 4c: alarm sounds generated deterministically (tools/sounds, IEC 60601-1-8 style bursts: high 10 pulses repeating, medium 3, low 2, technical 1) and played on the station from the alarm bus, highest priority first, stopped by acknowledge, silence or end; independent of any wristband |
| 2026-10-10 | (this push) | M4 step 5: wristband alert (WX-002, Immediate Alert Level, reviewed by Mohammad Irfan) and wristband targeting by crew member; M4 gate evidence written |
| 2026-10-10 | (this push) | M2 and M3 closed (CI run 38030464043 on 8550faf all green incl. release-windows); M5 started: localisation catalog, LevelGlyph, NoData/Stat (no number without a source), cards, AlarmBar, EscalationTracker, alarm store |
| 2026-10-10 | (this push) | Trace fix: with M3 enforced, F-ALM-01/02/03/08 must be cited by their tests; headers of the engine, ruleset, escalation and UI primitive tests now cite them (the previous push left trace.csv failing the gate) |
| 2026-10-10 | (this push) | M5 step 2: window states (enrolment, sign-in, lock, emergency, shell), role-based nav (ROLE_MATRIX §6), display settings (themes, text to 200 %, glove, reduced motion), first-administrator commands (CR-007 §4), auth_me no longer counts as activity, i18n gate test (no literal text outside the catalog) |
| 2026-10-10 | (this push) | M5 step 3: axe (WCAG 2.1 AA) and a keyboard-only walkthrough in Vitest browser mode (Chromium via Playwright) on every screen and theme and at 200 % with glove mode; the screenshot review found px type sizes, a truncated alarm title and focus lost after sign-in, all fixed and covered by tests; the trace and CI run the browser suite; Chromium for air-gapped checks is in the tool cache; M5 gate evidence |
| 2026-10-10 | (this push) | M6 step 1 (CR-012): signed configuration bundles (own signing domain, same keys and trust policy as the ruleset), crew roster apply (inactive, never deleted), user list/create/disable with re-authentication, Administration screen with the confirm step; roster fixtures signed with the development key; mojibake in three comments repaired |
| 2026-10-10 | (this push) | M6 step 2: read commands (vitals latest/trend, waveform, alarm list/detail/report F-ALM-09, device list/assign/unassign) with role and crew scope, MCC OPS fields only; alarm title stored (V5); device runtime (BLE supervise, notifications through the station synchronously, link → technical monitor), scan/pair, value-free observationsChanged/devicesChanged events; test-equipment scenario player and audit-tamper fault (CR-012 §8); UTC for display from the leap table |
| 2026-10-10 | (this push) | M6 step 2: read commands (vitals latest/trend, waveform, alarm list/detail/report F-ALM-09, device list/assign/unassign) with role and crew scope, MCC OPS fields only; alarm title stored (V5); device runtime (BLE supervise, notifications through the station synchronously, link → technical monitor), scan/pair, value-free observationsChanged/devicesChanged events; test-equipment scenario player and audit-tamper fault (CR-012 §8); UTC for display. trace.csv is regenerated at the end of the build-out (owner's choice: code first, tests after) |
| 2026-10-10 | (this push) | M4 and M5 closed (CI run 38036944297 on 7c782a7 all green). M6 step 3: Home, Body (vitals, ECG strip, trend with personal range), Alerts (list, why, timeline, actions, F-ALM-09 report; MCC status only), Devices (registry, assign with re-authentication, scan/pair), Audit (chain status, entries); Audit added to ROLE_MATRIX §6; crew selection and live reloads; content region keyboard-scrollable. M7, M9–M12 libraries being built in parallel worktrees |
| 2026-10-10 | (this push, [skip ci]) | GitHub Actions quota exhausted: pushes carry [skip ci]; gates needing CI stay open. Merged M9 (vehicle bridge), M10 (ground, path B, MDS, Open MCT plugin), M12 (advisory model runtime, SR-002 gate) libraries from parallel worktrees. M8 clinical records and Medical/Checks screens. Integration: vehicle readings stored per active crew member and evaluated (CO2/O2 environment alarms), signed vehicle/ground bundles, path B publishes every ground-stage escalation from the tick (technical alarm on failure), Environment and Ground screens, model status. Untested since the last full run: workspace clippy/nextest, new UI tests, browser suite (test pass at the end) |
| 2026-10-10 | (this push, [skip ci]) | Merged the MDS HTTPS transport (ureq + rustls/ring, TLS 1.3 mutual certificates, mission CA only; 62 caelum-mds tests pass); ISC allowed in deny.toml; third-party-risk rows for M8/M10/M12 crates. Export (FHIR R4, CSV). Kiosk display and waveform window (F-SYS-13). M6 end-to-end specs and UI tests for the M8–M12 screens written, not yet run. Open: MDS connector core wiring (durable outbox in the station database, [mds] signed configuration, client identity in the keystore, observation enqueue); retention needs nothing until a retention class other than Mission exists |
| 2026-10-10 | (this push, [skip ci]) | Full local test pass: workspace clippy clean (default and test-equipment), 457 workspace + 71 test-equipment Rust tests pass, UI typecheck/lint clean, 186 UI unit tests and 98 browser checks (axe WCAG 2.1 AA, 200 %/glove, keyboard walkthrough; now including Medical, Checks, Environment, Ground, kiosk) pass, guards and 97 self-tests pass, cargo-deny licences/bans/sources ok, trace 591 tests / FAIL 0. Fixes: clippy findings, dead receiver variable, test fixtures for kiosk/mdsStatus, ISC aligned in the npm guard. Not run: the tauri-driver end-to-end specs (need the e2e toolchain installed) |
| 2026-10-10 | (this push, [skip ci]) | M7 finished by the lead from the stopped agent's work and merged: signal processing (QRS, HRV, SQIs, respiration, SpO2 from PPG, core temperature, PAT/BP trend, activity/sleep, baselines), GHS and sensor-kit decoders, PhysioNet reference excerpts, CR-013. Integrated: GHS observations through acquisition, ECG chunks stored and run through the streaming QRS detector (qrs events drive asystole.critical; SQI and heart rate stored as derived with algorithm versions), baselines estimated every 10 min and given to the engine. Safety fix found by the integration test: an unusable gate reading (lead off) now closes the absence gate at once, so asystole cannot fire on the last good SQI (engine; hazard H-03 updated). 547 workspace tests pass; clippy clean. GHS characteristic 0x2B8B to be confirmed on the HIL bench |
| 2026-10-10 | 20f119a, fc29e4e ([skip ci]) | Merged M11 caelum-cluster (fail-over, crew LAN; CR-017, docs/integration/M11.md; third-party-risk rows for its network crates) and M8 file formats (DICOM via dicom-rs pure-Rust codecs, EDF/EDF+ read and write, HL7 aECG, SCP-ECG, FHIR R4 import, CSV export; CR-014); status footer shows device links, path B and instance role. Workspace clippy clean, 596 tests pass |
| 2026-10-10 | (this push, [skip ci]) | M8 import wiring: DICOM and ECG files must open before they are kept (identity and leads in the metadata); FHIR import through the ingest checks, stored as imported and evaluated, original kept, audited (`fhir.import`); ECG review with keyboard calipers (F-CLN-03) and DICOM viewer with VOI windowing in the core (F-CLN-04); commands fhir_import, ecg_view, dicom_view (CMO, Flight Surgeon). Tests: 3 station + 5 renderer tests, 4 UI tests; 604 workspace and 82 test-equipment Rust tests pass, clippy clean in both builds, guards pass, viewers added to the browser suite (108 checks pass), 190 UI unit tests pass; stale GroundStatus fixtures fixed (instanceRole). Status tables brought up to date |
| 2026-10-10 | (this push, [skip ci]) | Trace regenerated after the M7, M11 and M8 merges: 133 IDs, 734 tests, PASS 97, NO_TEST 36, FAIL 0, NOT_RUN 0; enforced milestones M0–M5 pass |
| 2026-10-10 | (this push, [skip ci]) | M11 core wiring, step 1: V8 migration (cluster_log, cluster_quarantine, cluster_state); `caelum-core::cluster` (signed `cluster` bundle with peer, stations and keys; DbLogStore and DbFenceStore on the encrypted database; LanAuthority on the station's accounts; LanAlarms from the alarm bus); commands config_apply_cluster and cluster_identity_set (administrator); the app starts the node when configured and reports the real role; failures raise `technical.cluster` (added to the CR-011 table with `technical.path_b` and `technical.mds_unreachable`); CR-017 §4 provisioning decision. 7 new tests, including a real node on the encrypted stores. Open: replication of station writes (step 2) |
| 2026-10-10 | (this push, [skip ci]) | CR-020 applied: full local pass on b285b40 (clippy both builds; 616 + 94 Rust tests; 190 UI and 108 browser checks; guards; cargo deny; trace 133 IDs / 745 tests, PASS 107, NO_TEST 26, FAIL 0). Guard self-tests found the new advisory ignore missing from `.cargo/audit.toml` and the register's advisory table: fixed. Gate records written for M6–M12 (`docs/gates/`); **M9 complete** (both gate conditions met; correlation view F-ENV-04 added; `spe_state` waits for ICD-VEH OI-2). M6, M7, M8, M10, M11, M12 stay in progress; each gate file lists exactly what remains. Untested IDs left: F-ACQ-12/13/14, F-CLN-09/10, F-GND-04/08/10, F-ML-02/03, F-SYS-12, R-ALM-03, R-INT-02, R-OPS-01, SR-013, N-01..N-10 (later releases, unbuilt M10/M12/M13 work, usability and platform evidence) |
| 2026-10-10 | (this push, [skip ci]) | M7 integration completed (CR-021): sensor kit scanned, paired and connected (`ble-sensor-kit`); frames through acquisition (sequence gaps and duplicates, readings through ingest); station signal hub with contiguous buffers: respiration from RIP (30 s) and from the ECG when there is no RIP band, PPG pulse rate and `sqi_ppg` (8 s), activity and sleep/wake per 60 s epoch (`sleep_stage` gains unstaged `sleep`), core temperature started only from a measured core-equivalent temperature. No SpO₂ from kit PPG and no PAT/BP trend until ICD OI-3/OI-4. WX-001 now also names 0x2B8B (missing since M7) and the kit Data characteristic; review requested. 7 new tests pass; the full test pass comes after the build-out (owner's direction) |
| 2026-10-10 | (this push, [skip ci]) | M11 built out: row replication (V9 capture triggers, signed row entries, standby upsert with the primary's audit chain kept apart, seed once), passive standby (no tick or intake, writes refused) and promotion through the restart path (3 tests pass); crew stations: signed `crew-station` bundle, LAN client following the primary, secondary sign-in on the primary, kiosk emergency view, Crew station screen. Gate run (kill the primary under load) is in the test phase |
| 2026-10-10 | (this push, [skip ci]) | M8 build-out (CR-022): retention (Technical class, signed limits, alarm guard, audited), DICOM folder watch (signed folder and crew member, moved aside, never deleted), ECG events and EDF+ segment export, encounter attachment picker, signed check schedule with the next check shown. Tests written, run in the test phase |
| 2026-10-10 | (this push, [skip ci]) | M10 onboard downlink (CR-024): optional `ground_link` in the signed ground configuration; MEDICAL summaries per window and alarm event packets (CFDP class 2) queued, signed, persisted, sent only inside signed contact windows (`config_apply_contact`); ACKs received; `technical.ground_link` on failure; Ground screen shows the downlink. Test written for the test phase |
