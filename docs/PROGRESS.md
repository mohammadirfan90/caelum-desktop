# CAELUM Desktop: progress and context

Living record of where the project stands, why things are the way they are, what is open, and how
to continue. Updated with every push (the commit history holds the details; this file holds the
picture). Last updated: 2026-10-10 (M3 independent review recorded; CI runs no longer cancelled).

## 1. Milestones
| Milestone | State | Evidence | Open |
|---|---|---|---|
| M0 Repository and tooling | **complete** (13292b4) | `docs/gates/M0.md`, CI run 38023371185 | Physical network-off install test moved to M13 (CR-005) |
| M1 Domain, storage, security core | **complete** (13292b4) | `docs/gates/M1.md` | First-administrator enrolment arrives with the M3/M6 login screen (CR-007 §4) |
| M2 Ingest pipeline and device framework | code complete; closes on the next all-green CI run | `docs/gates/M2.md` | Physical devices on the hardware-in-the-loop bench (docs/09) |
| M3 Alarm engine | gate items met; closes on the next all-green CI run | `docs/gates/M3.md` (independent review: Mohammad Irfan, 2026-10-10) | ⚕ clinical content stays DRAFT |
| M4 Escalation, technical alarms, notifications, self-test | gate items met; closes on the next all-green CI run | `docs/gates/M4.md` | Wristband against a real device on the HIL bench; device runtime wiring with the Devices screen (M6) |
| M5–M13 | not started | | |

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
