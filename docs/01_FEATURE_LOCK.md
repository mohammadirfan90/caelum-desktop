# 01. Feature lock: CAELUM Desktop v1.0

**Status: LOCKED** (rev. 3 via CR-002: adds F-SYS-13; clarifies F-ACQ-01, F-ACQ-10, F-ALM-04,
F-GND-11, F-GND-13, F-SYS-06 and the platforms. Rev. 2 via CR-001: adds F-GND-12..14, onboard
medical data system integration). Priority: **M** = must (v1.0 cannot ship without it), **S** = should (v1.0 target,
may slip to v1.1 only by change request), **v1.1 / v2** = explicitly not in v1.0.

## Change control
A change request (`docs/change-requests/CR-NNN.md`) states the feature, reason, affected
requirements, safety impact and test impact. It needs approval from the product owner and, for
anything touching alarms or clinical content, the medical lead. Claude Code may draft a CR but
must never implement an unapproved one. Until a licensed clinician advisor countersigns, the
medical lead (Abdur Rahman FahAd) is provisional (CR-003): clinical items they approve are DRAFT,
rulesets are signed with development keys only, and the "DEVELOPMENT RULESET" banner shows.

## Platforms and roles
- **Platforms:** Windows 11 x64 and Windows 10 LTSC x64 (reference; NASA host baseline),
  Ubuntu 22.04+ x64, macOS 13+ (development and ground use). Supported Windows 10 LTSC editions
  (checked 2026-10-09): IoT Enterprise LTSC 2021 (support until 2032-01-13), Enterprise LTSC
  2019 and IoT Enterprise LTSC 2019 (until 2029-01-09). Enterprise LTSC 2021 (non-IoT) ends
  2027-01-12 and is not supported.
- **Roles:** Astronaut, Crew Medical Officer (CMO), Flight Surgeon (ground), Biomedical Engineer
  (BME, ground), Mission Control Operator (ground, operational status only), Administrator,
  Device Gateway (machine identity).
- **Deployment profiles:** `onboard-primary`, `onboard-standby`, `ground` (flight surgeon / BME
  console), `analog` (habitat and bed-rest studies).

## F-ACQ: Device acquisition and integration
| ID | Feature | Pri |
|---|---|---|
| F-ACQ-01 | Native BLE central: scan, pair and bond (platform pairing modules; guided OS pairing as fallback), reconnect automatically, continuous background acquisition | M |
| F-ACQ-02 | Bluetooth SIG profiles: Heart Rate, Pulse Oximeter (spot + continuous), Blood Pressure, Health Thermometer, Weight Scale, Glucose | M |
| F-ACQ-03 | Bluetooth Generic Health Sensor (GHS, IEEE 11073-10206 ACOM), including ECG waveform | M |
| F-ACQ-04 | USB / serial adapter framework (CDC-ACM, HID) with per-device decoders | M |
| F-ACQ-05 | CAELUM sensor shirt and wristband protocol (ECG, RIP respiration, IMU, temperature, PPG, EDA) | M |
| F-ACQ-06 | DICOM import (file drop, folder watch, DICOM C-STORE SCP) for ultrasound and images | M |
| F-ACQ-07 | ECG recording import: EDF/EDF+, HL7 aECG, DICOM waveform, SCP-ECG | M |
| F-ACQ-08 | HL7 FHIR R4 import (Observation, Device, DiagnosticReport) | M |
| F-ACQ-09 | Device registry: model, serial, firmware, accuracy class, calibration due, assignment to crew | M |
| F-ACQ-10 | Device health: battery, signal quality, link state and link metrics (RSSI, packet loss %, reconnect count), clock offset, dropped-packet counters | M |
| F-ACQ-11 | Time synchronisation of all devices to vehicle time (MET / TAI / UTC), offsets recorded | M |
| F-ACQ-12 | Adapter plug-in SDK (versioned Rust trait, isolated process option, signed manifests) | S |
| F-ACQ-13 | Smart-garment vendor adapters (Astroskin-class) via plug-in SDK | S |
| F-ACQ-14 | IEEE 11073 SDC consumer (monitors, defibrillators; read-only) | v1.1 |

## F-SIG: Signal processing (onboard, deterministic)
| ID | Feature | Pri |
|---|---|---|
| F-SIG-01 | ECG: filtering, QRS detection, RR, HR, HRV (time and frequency domain) | M |
| F-SIG-02 | Signal quality indices for ECG, PPG, respiration; drives alarm gating | M |
| F-SIG-03 | Respiration rate and pattern from RIP and ECG-derived respiration | M |
| F-SIG-04 | SpO₂ and pulse rate from PPG with motion rejection | M |
| F-SIG-05 | Core temperature estimate (heat-flux / HR Kalman model) | M |
| F-SIG-06 | Pulse arrival time and cuff-calibrated BP trend | S |
| F-SIG-07 | Activity and sleep/wake from physiology (microgravity-safe, no posture assumption) | M |

## F-ALM: Alarms and escalation (safety-critical)
| ID | Feature | Pri |
|---|---|---|
| F-ALM-01 | Deterministic rule engine: absolute limits, persistence, hysteresis, supersession, data-gap handling | M |
| F-ALM-02 | Signed, versioned ruleset (TOML) with approver record; refuses to start on bad signature | M |
| F-ALM-03 | Alarm priorities per IEC 60601-1-8 concepts (high / medium / low + advisory), shape-coded | M |
| F-ALM-04 | Escalation: astronaut → CMO → sound (crew stations always; wristband Immediate Alert in addition) → whole crew → ground (OPS channel, path B), with timers | M |
| F-ALM-05 | Acknowledge, silence (time-limited), escalate manually, resolve, with audit | M |
| F-ALM-06 | Technical alarms: sensor off, low battery, device lost, stale data, clock drift, storage low | M |
| F-ALM-07 | Crew-wide emergency view on every crew station, with location and protocol | M |
| F-ALM-08 | Personal-baseline deviation notices (observe level only, never suppress) | M |
| F-ALM-09 | Alarm history, statistics and alarm-fatigue report | S |

## F-CLN: Clinical workflows
| ID | Feature | Pri |
|---|---|---|
| F-CLN-01 | Crew overview for CMO and flight surgeon (all crew, status, trends) | M |
| F-CLN-02 | Medical protocol library as interactive checklists linked to alarms | M |
| F-CLN-03 | 12-lead / long ECG review: waveform viewer, calipers, events, export | M |
| F-CLN-04 | Ultrasound and image viewer (DICOM), attach to encounters and event packets | M |
| F-CLN-05 | Encounters: SOAP notes, vitals snapshot, attachments, sign-off | M |
| F-CLN-06 | Scheduled checks: resting vitals, PVT reaction test, symptom questionnaire, guided device sessions | M |
| F-CLN-07 | Daily private check-in (mood, symptoms, sleep); privacy-tiered | M |
| F-CLN-08 | Medication and medical-kit log | S |
| F-CLN-09 | Behavioural-health tools beyond check-in (coach, contested patterns) | v2 |
| F-CLN-10 | Digital twin, bone, vision (SANS), immune modules | v2 |

## F-ENV: Environment and exposure
| ID | Feature | Pri |
|---|---|---|
| F-ENV-01 | Read-only vehicle telemetry: CO₂, O₂, pressure, temperature, humidity | M |
| F-ENV-02 | Radiation: personal and area dosimetry, cumulative dose, solar-particle-event state | M |
| F-ENV-03 | Exercise device sessions (load, duration) for fitness and countermeasure tracking | S |
| F-ENV-04 | Correlation view: environment events alongside physiology | S |

## F-ML: Models (advisory)
| ID | Feature | Pri |
|---|---|---|
| F-ML-01 | ONNX runtime with model registry, hash pinning, model cards, validation status shown in UI | M |
| F-ML-02 | Models M1 signal quality, M2 rhythm (AF), M4 sleep, M9 personal-baseline anomaly | M |
| F-ML-03 | Models M3 respiration, M5 stress, M6 BP trend, M7 fitness, M8 short-term risk | S |
| F-ML-04 | Explanations: top contributing features for every model output | M |

## F-GND: Ground and mission-control integration
| ID | Feature | Pri |
|---|---|---|
| F-GND-01 | CCSDS Space Packets (133.0-B-2) with CUC time, APID plan, sequence counts | M |
| F-GND-02 | XTCE mission database export for every downlinked parameter | M |
| F-GND-03 | Yamcs link: UDP/TCP packet stream to a Yamcs instance; health dictionary | M |
| F-GND-04 | Open MCT plugin package: telemetry provider, crew status board, alert timeline | M |
| F-GND-05 | Store-and-forward downlink queue with priorities, per-channel allow-lists, signing | M |
| F-GND-06 | CFDP (727.0-B-5) file transfer for event packets, reports, DICOM and ECG files | M |
| F-GND-07 | Uplink of approved items only: ruleset updates, protocol library, model packages (signed) | M |
| F-GND-08 | Ground console profile: flight-surgeon and BME views, private medical conference record | M |
| F-GND-09 | HL7 FHIR R4 export / Bundle to medical record systems (incl. NASA Medical Data Architecture) | M |
| F-GND-10 | Delay-tolerant networking (Bundle Protocol v7) adapter | v1.1 |
| F-GND-11 | cFS bridge: subscribe to vehicle telemetry (read-only, `caelum-vehicle`); publish CAELUM health status packets (telemetry only, `caelum-ground`; entry into cFS per ICD-VEH) | M |
| F-GND-12 | Onboard medical data system connector (MDA-class): two-way FHIR R4 read/write, attachment hand-off, sync state, connection health | M |
| F-GND-13 | Dual outbound routing: (A) medical-record path via the medical data system or direct CFDP fallback; (B) operational status/alert path always direct to the vehicle (cFS per ICD-VEH), never via the medical data system | M |
| F-GND-14 | Summary-first data policy: summaries, alerts, notes and event-linked files sent by default; raw waveforms only on event or ground request | M |

## F-SYS: Platform, security and operations
| ID | Feature | Pri |
|---|---|---|
| F-SYS-01 | Role-based login, auto-lock, re-authentication for sensitive actions, emergency break-glass | M |
| F-SYS-02 | Encrypted database, keys in OS keystore or hardware token | M |
| F-SYS-03 | Signed, hash-chained audit log covering every clinical and configuration action | M |
| F-SYS-04 | Primary/standby fail-over over crew LAN, ≤ 10 s takeover, no acknowledged data loss | M |
| F-SYS-05 | Crew-station LAN service: other laptops/tablets join as secondary displays | M |
| F-SYS-06 | Offline install, signed updates delivered by uplink or media (no internet updater); release builds reproducible offline from a prepared, checksummed tool cache | M |
| F-SYS-07 | Configuration as signed files (devices, crew, channels, APIDs) | M |
| F-SYS-08 | Data retention, export (FHIR, CSV, EDF+), de-identified research export | M |
| F-SYS-09 | Self-test at start and continuous watchdogs (storage, clock, tasks, devices) | M |
| F-SYS-10 | Accessibility: glove mode, text up to 200 %, high contrast, keyboard only, screen reader | M |
| F-SYS-11 | Localisation framework (English v1.0; Russian, Japanese, French, German strings v1.1) | S |
| F-SYS-12 | Resource budget mode for constrained hosts | S |
| F-SYS-13 | Kiosk mode for a wall-mounted crew display; multiple windows (e.g. waveform viewer on a second monitor) | S |

## Out of scope for v1.0 (explicit)
Cloud services of any kind; commanding any vehicle system (publishing CAELUM status telemetry per
ICD-VEH is not commanding); controlling therapeutic devices;
image or ultrasound diagnosis by AI; a mobile app; billing; social features; any data shown that
did not come from a device, a crew entry or an approved import.
