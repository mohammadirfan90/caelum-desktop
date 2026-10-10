# CAELUM Desktop

Onboard crew health monitoring for long-duration spaceflight. CAELUM is a Tauri 2 desktop
application with a Rust core. It:

- connects medical and wearable devices;
- evaluates crew health with deterministic, signed safety rules;
- escalates alarms to the crew and to ground;
- exchanges data with mission control using CCSDS, XTCE, Yamcs, Open MCT and HL7 FHIR.

It works with no ground link, never commands the vehicle or a therapeutic device, and never
shows a value it does not have: a missing or stale value is shown as NO DATA, STALE or SIGNAL
UNUSABLE.

> **Status (2026-10-10).** Version 0.1.0, under development. Milestones M0–M5 and M9 are
> complete. M6–M8 and M10–M12 are built, with gate runs still open. M13 (release readiness) has
> not started. Clinical content is DRAFT under a provisional medical lead (CR-003). No onboard
> model has been trained or packaged yet (see [Models](#5-onboard-models-advisory-only)).
> Project state: [`docs/PROGRESS.md`](docs/PROGRESS.md). Gate records: [`docs/gates/`](docs/gates/).

---

## 1. High-level architecture

```
 Wearables & medical devices            Vehicle (cFS, read-only subscribe)
 BLE SIG profiles, GHS ECG,             CO₂, O₂, pressure, temperature, humidity,
 CAELUM sensor kit, serial/HID,         radiation dose, exercise sessions, vehicle time
 DICOM / ECG / FHIR files                         │
            │                                     │
            ▼                                     ▼
 ┌──────────────────────── CAELUM instance (one OS process) ─────────────────────────┐
 │ adapters → ingest → signal processing → alarm engine → escalation → notifier      │
 │   (decode)  (units,    (QRS, SQI, HR,      (signed,          (stages, sound,       │──► crew stations
 │             time,       respiration,        deterministic     wristband)           │    (always sound)
 │             plausibility) activity, ...)    rules)                                 │
 │ encrypted record (SQLCipher) · signed hash-chained audit · advisory models (ONNX)  │
 └───────┬──────────────────────────────┬──────────────────────────┬─────────────────┘
         │ (A) medical record            │ (B) operational status    │ crew LAN (mTLS)
         ▼ FHIR R4 (HTTPS, mTLS)         ▼ CCSDS TM, OPS fields only  ▼
  Onboard medical data system      Vehicle → ground (Yamcs/MCS)   standby instance (fail-over),
  (MDA-class) ⇄ ground record      → Open MCT consoles            secondary displays, kiosks
         ▲
         └─ (A′) fallback: MEDICAL summaries and event packets by CFDP in contact windows
```

- **Path A (medical record):** complete clinical data to the onboard medical data system over
  FHIR R4. The data is kept and resent if that system is unreachable. Read-back observations are
  stored as `imported` and evaluated.
- **Path B (operational):** small status and alarm telemetry with **no physiology** (no value,
  parameter name, rule ID or free text). It is sent straight to the vehicle as telemetry-only
  CCSDS packets. Alarms never depend on the medical data system (SR-015).
- **Ground downlink (A′):** MEDICAL parameter summaries and alarm event packets. They are signed
  and persisted, then sent only inside signed contact windows, with files by CFDP class 2.
- **Fail-over:** a primary and a standby on the crew LAN.
  - The primary replicates every write of the record as signed, hash-chained log entries.
  - The standby is passive until it is promoted (≤ 10 s, epoch fencing). It then resumes open
    alarms at their escalation stage.
  - Secondary displays sign users in on the primary. Kiosks show the crew-wide emergency view
    without sign-in.
- **Safety boundaries** (enforced by crate boundaries and CI guards):
  - read-only to the vehicle and to therapeutic devices;
  - every device write is listed in [`docs/write-exceptions.md`](docs/write-exceptions.md);
  - release builds cannot contain test equipment.

## 2. Low-level architecture

### 2.1 Cargo workspace
| Crate | Role |
|---|---|
| `caelum-domain` | Parameter catalog (44 parameters, single source of truth), units, IDs, TAI/UTC/MET time with the IERS leap table. No I/O. |
| `caelum-store` | SQLCipher 4 database (key from the OS keystore). Migrations V1–V9, repositories, row replication, retention purge. |
| `caelum-ingest` | Validation, explicit unit conversion, plausibility, clock correction, provenance. Rejection codes; never a default. |
| `caelum-devices` | Device registry, BLE central, OS pairing, Bluetooth SIG decoders, GHS (IEEE 11073-10206), CAELUM sensor kit, read-only serial/HID. File formats: DICOM, EDF/EDF+, HL7 aECG, SCP-ECG, FHIR R4, CSV. |
| `caelum-signal` | Pure, deterministic signal processing (§3). |
| `caelum-alarms` | Signed TOML ruleset, deterministic engine, escalation timeline, technical alarms. |
| `caelum-ml` | Signed model packages, registry, feature builders, time-limited ONNX runtime, observe-only gate (§5). |
| `caelum-vehicle` | cFS Software Bus subscriber (no send API), vehicle time correlation. |
| `caelum-ground` | CCSDS Space Packets with CUC time, XTCE generator, store-and-forward queue, CFDP, uplink verification, FHIR export, path B publisher. |
| `caelum-mds` | FHIR R4 connector to the onboard medical data system: outbox and inbox, health, HTTPS with mutual TLS 1.3. |
| `caelum-security` | Roles and the command access table, argon2id, sessions and break-glass, keystore, Ed25519 instance key, signed audit chain, attachment encryption (AES-256-GCM). |
| `caelum-cluster` | Fail-over state machine, signed log replication, crew-LAN service (axum, rustls). |
| `caelum-core` | Wiring: the station (acquisition, signal hub, alarm service, retention, downlink, replication, provisioning). |
| `caelum-test-equipment` | Device emulators, scenario player, fault injection. Behind the `test-equipment` Cargo feature only. |
| `src-tauri` | Thin IPC commands (typed with tauri-specta), windows, runtimes (devices, vehicle, MDS, cluster, crew LAN). |
| `ui` | React 19 + TypeScript + Tailwind v4. Screens for crew, medical, devices, environment, ground and audit. WCAG 2.1 AA, 200 % text, glove mode. |
| `plugins/openmct` | Open MCT plugin: telemetry provider, crew status board, alert timeline. |

### 2.2 Data path and timing
1. **Acquisition.** An adapter decodes bytes into readings with device time and quality.
2. **Ingest.** Clock correction, unit conversion, plausibility and provenance turn each reading
   into an observation. Device-flagged demonstration or test data is rejected in release builds
   (SR-017).
3. **Storage.** Observations are stored. Waveforms are stored as 1 s chunks.
4. **Signal processing.** Derived observations are produced, each tagged `derived` with its
   algorithm version.
5. **Alarm engine.** Every observation is evaluated synchronously in the ingest path, with no
   queue between data and safety logic. A 4 Hz tick runs escalation and absence rules such as
   asystole.
6. **Fan-out.** Value-free events go to the UI, OPS records to path B, summaries and event
   packets to the ground queue, rows to the replication log, and entries to the audit chain.

Budgets: device sample to UI ≤ 500 ms; persistence met to alarm on every crew station ≤ 1 s.

### 2.3 Safety layer (alarm engine)
- **Ruleset:** one signed TOML file, verified at start and again when uplinked. A ruleset that
  fails verification falls back safely and raises a technical alarm (SR-009).
- **Rule types:**
  - continuous thresholds with persistence and hysteresis;
  - spot readings that need confirmation (SR-019);
  - absence of events (asystole);
  - personal-baseline notices, which are observe level only;
  - supersession within a group (SR-006).
- **Absolute limits** are never suppressed or delayed by baselines or models (SR-002, R-ALM-06).
- **Signal quality gates:** a poor or unusable signal pauses its rules and raises a technical
  alarm instead (SR-005).
- **Escalation:** astronaut and CMO at 0 s, then sound, crew and ground on fixed timers. Silence
  is time-limited, latched alarms stay until acknowledged, and escalation resumes at the right
  stage after a restart or a fail-over (SR-007, SR-008).

## 3. Signal-processing algorithms (`caelum-signal`)
Every output carries an algorithm ID. When an output cannot be computed from the data, the
function returns no number.

| Output | Algorithm | Method |
|---|---|---|
| QRS, R-R, heart rate | `qrs-pan-tompkins/1` | Band-pass, derivative, squaring, moving-window integration, adaptive thresholds with search-back |
| ECG signal quality | SQI family | Template correlation, power ratios, flat-line and clipping checks |
| Respiration rate (RIP) | `resp-count-adv/1` | Schäfer & Kratky advanced counting, band-pass 0.05–1.5 Hz |
| Respiration rate (ECG-derived) | `resp-edr-ramp/1` | R-amplitude modulation resampled at 4 Hz, same counting |
| Pulse rate (PPG) | `pr-ppg-acf/1` | Autocorrelation of the 0.5–5 Hz infrared component |
| SpO₂ (PPG) | `spo2-ratio-of-ratios/1` | Ratio of ratios with the **device's own** calibration curve; none, no SpO₂ |
| Core temperature | `core-temp-ekf-hr-hf/1` | Extended Kalman filter with heart rate (Buller et al. 2013), started only from a measured core-equivalent temperature |
| Activity, sleep/wake | `activity-motion/1`, `sleep-wake-actigraphy-hr/1` | High-pass accelerometer magnitude (no posture or 1 g assumed); 60 s epochs |
| Pulse arrival time, BP trend | `pat-maxslope/1`, `bp-trend-pat-inverse/1` | R peak to PPG maximum slope; `SBP = a + b/PAT` fitted to cuff readings |
| Personal baselines | robust median/MAD | Per astronaut, at rest, from their own data only |

Accuracy is checked on PhysioNet reference records (MIT-BIH 100, 105, 119, 232; PhysioNet/CinC
Challenge 2015 asystole alarms) in the test suite, with the excerpts included under ODC-By 1.0.

## 4. Interfaces and standards
- **Devices:**
  - Bluetooth SIG profiles (HRS, PLXS, BPS, HTS, WSS, GLS);
  - IEEE 11073-10206 GHS;
  - the CAELUM sensor kit (`docs/icd/ICD-SENSOR-KIT.md`);
  - DICOM, EDF/EDF+, HL7 aECG, SCP-ECG and FHIR R4 files.
- **Vehicle and ground:**
  - cFS (subscribe only);
  - CCSDS 133.0-B-2 Space Packets with CUC time;
  - CCSDS 727.0-B-5 CFDP;
  - XTCE for Yamcs;
  - an Open MCT plugin;
  - HL7 FHIR R4 to the medical data system.
- **NASA and international standards the requirements trace to**
  ([`docs/02_NASA_REQUIREMENTS.md`](docs/02_NASA_REQUIREMENTS.md)):
  - NASA-STD-3001 Vol. 1 (crew health) and Vol. 2 (human factors, caution and warning);
  - NPR 7150.2 (software engineering);
  - NASA-STD-8739.8 (software assurance and safety);
  - NASA-STD-1006 (space system protection);
  - IEC 60601-1-8 (alarms), IEC 62304, IEC 62366-1 and ISO 14971.
- **No NASA physiological datasets are bundled or used to compute any value.** The standards
  above govern requirements and design. Thresholds come from the signed ruleset under the
  medical lead, and baselines come from each astronaut's own data.

## 5. Onboard models (advisory only)
### 5.1 Model runtime architecture (`caelum-ml`, built)
```
signed package ──► verify (Ed25519, SHA-256 of model, flight builds need a medical approver)
  (ONNX file, model card, feature spec)      │ refused → listed DISABLED with the reason
                                             ▼
registry ── feature builder ── ONNX Runtime (ort, load-dynamic, pinned checksummed library,
 (per id)   (windowed features    CPU, fixed threads, hard time limit per inference)
            from stored readings;          │
            POOR/UNUSABLE = gaps)          ▼
                               range checks ─► AdvisoryResult (value, explanation, status)
                                                       │
                       observe-only gate ◄─────────────┘ (only into observe-level rules)
                       never: thresholds, acknowledge, silence, escalation (SR-002)
```
- Validation status is shown as text: `VALIDATED`, `INVESTIGATIONAL`, `DISABLED` or
  `UNAVAILABLE`. Development-signed packages carry a DEVELOPMENT MODEL badge.
- SR-002 is tested with adversarial model outputs: absolute-limit decisions never change.

### 5.2 Planned models ([`docs/08_ML_SPEC.md`](docs/08_ML_SPEC.md))
**None of these has been trained, evaluated or packaged.** The training pipeline (`ml/`) is not
in this repository yet, and the app ships with no model. The table states plans and gates, not
results.

| ID | Purpose | Inputs | Planned training data (public) | Gate before it may ship |
|---|---|---|---|---|
| M1 | Signal quality | ECG, PPG, respiration windows, IMU | MIT-BIH and Noise Stress Test, PPG-DaLiA | AUROC ≥ 0.90 on held-out subjects |
| M2 | Rhythm (AF and others) | single-lead ECG, 30 s | Icentia11k, PhysioNet/CinC 2017, MIT-BIH | AF F1 ≥ 0.80, subject-wise |
| M3 | Respiration events | RIP, ECG-derived respiration | BIDMC, CapnoBase, Apnea-ECG | rate MAE < 2 /min |
| M4 | Sleep staging | HR, HRV, respiration, temperature, activity (no posture) | DREAMT, Sleep-Accel, MESA | sleep/wake ≥ 85 % |
| M5 | Stress index | HRV, EDA, respiration, temperature | WESAD | F1 ≥ 0.80, leave-one-subject-out |
| M6 | BP trend | PAT, PPG morphology | PulseDB, VitalDB | trend direction ≥ 85 % after calibration |
| M7 | Fitness and recovery | HR versus workload | PhysioNet treadmill exercise tests | VO₂max error < 10 % |
| M8 | Short-term deterioration risk | 30–60 min of vitals | MIMIC-IV, VitalDB | ≥ 15 min lead time at an agreed false-alert rate |
| M9 | Personal-baseline anomaly | daily feature vector | each astronaut's own pre-flight baseline | injected drifts detected in ≤ 3 days, < 1 false notice per week |

M2 may feed only the observe-level `af.observe` rule until it is `VALIDATED`. Promoting it
further needs a change request.

**Spaceflight-specific effects.** Effects such as altered thermoregulation, fluid shift, exercise
motion artefact and circadian disruption are relevant to these models. Their size must come from
cited, verified sources reviewed by the medical lead, and they must be evaluated on frozen
evaluation sets before any claim is made. They are not encoded anywhere in CAELUM today.

## 6. Security and data protection
- **Records:** SQLCipher 4, with the database key in the OS keystore. Attachments use
  AES-256-GCM.
- **Audit:** every security and clinical action is recorded in a hash-chained audit log, each
  entry signed with the instance Ed25519 key.
- **Commands:** every IPC command and LAN endpoint is checked by role and crew scope
  ([`docs/ROLE_MATRIX.md`](docs/ROLE_MATRIX.md)). Mission control (OPS) never receives
  physiology.
- **Signed configuration:** every configuration (ruleset, roster, protocols, vehicle, ground,
  contact schedule, MDS, cluster, crew station, retention, import, schedule) is a signed bundle.
  It is verified when applied and again when used. Release builds accept only flight keys.
- **Break-glass:** the emergency view opens without sign-in, and the opening is audited and
  notified (SR-014).

## 7. Build and test
```bash
pnpm install                                     # UI dependencies
cargo nextest run --workspace                    # Rust tests
cargo nextest run -p caelum-store -p caelum-ingest -p caelum-core --features test-equipment
cargo clippy --workspace --all-targets -- -D warnings
pnpm -C ui typecheck && pnpm -C ui test && pnpm -C ui test:a11y
pnpm -C tools/ci check && pnpm -C tools/ci test  # release, write-exception and licence guards
cargo deny check
pnpm -C tools/ci trace                           # all suites → docs/trace.csv, milestone enforcement
pnpm tauri dev                                   # run the desktop app
```
- **Release builds** are reproducible offline from a checksummed tool cache
  ([`docs/OFFLINE_BUILD.md`](docs/OFFLINE_BUILD.md)).
- **CI is currently off:** milestone gates use the full local pass under CR-020. A CI run on
  `main` is required before any release.

## 8. Documentation map
| Document | Content |
|---|---|
| [`CLAUDE.md`](CLAUDE.md) | Project rules, stack, commands, definition of done |
| [`docs/01_FEATURE_LOCK.md`](docs/01_FEATURE_LOCK.md) | Locked feature set with IDs and priorities |
| [`docs/02_NASA_REQUIREMENTS.md`](docs/02_NASA_REQUIREMENTS.md) | Standards and the requirement-to-feature trace |
| [`docs/03_ARCHITECTURE.md`](docs/03_ARCHITECTURE.md) | Process model, crates, data flow, storage, security, fail-over |
| [`docs/04_INTEGRATIONS.md`](docs/04_INTEGRATIONS.md) | Devices, vehicle, mission control, medical records |
| [`docs/05_DATA_MODEL.md`](docs/05_DATA_MODEL.md) | Schema, parameter catalog, provenance, retention |
| [`docs/06_SAFETY_AND_ALARMS.md`](docs/06_SAFETY_AND_ALARMS.md) | Alarm system, escalation, hazards, safety requirements |
| [`docs/07_UI_SPEC.md`](docs/07_UI_SPEC.md) | Design system, screens, accessibility |
| [`docs/08_ML_SPEC.md`](docs/08_ML_SPEC.md) | Model rules, planned models, pipeline |
| [`docs/09_VV_PLAN.md`](docs/09_VV_PLAN.md) | Verification and validation, performance budgets |
| [`docs/10_BUILD_PLAN.md`](docs/10_BUILD_PLAN.md) | Milestones M0–M13 and their gates |
| [`docs/PROGRESS.md`](docs/PROGRESS.md) | Current state, decisions, open items, change log |
| [`docs/gates/`](docs/gates/) | Gate evidence per milestone |
| [`docs/change-requests/`](docs/change-requests/) | Every design decision (CR-001 to CR-024) |
| [`docs/integration/`](docs/integration/) | Per-milestone wiring notes |
| [`docs/icd/`](docs/icd/) | Interface control documents (vehicle, medical data system, sensor kit) |
| [`docs/ROLE_MATRIX.md`](docs/ROLE_MATRIX.md) | Every command and endpoint, by role |
| [`docs/write-exceptions.md`](docs/write-exceptions.md) | Every permitted device write |
| [`docs/third-party-risk.md`](docs/third-party-risk.md) | Third-party software (SOUP) risk register |
| [`docs/trace.csv`](docs/trace.csv) | Requirement-to-test traceability |

## 9. Licence
Apache-2.0 ([`LICENSE`](LICENSE), [`NOTICE`](NOTICE)). Copyright ChronoStrider.
