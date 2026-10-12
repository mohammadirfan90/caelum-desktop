# 02. NASA and international requirements

Always use the current revision of each document. Items marked *(hardware)* apply to the sensor
kit, not to this software, and are listed so the system-level trace is complete.

## 1. Source documents
| Area | Document | Use in CAELUM |
|---|---|---|
| Crew health | NASA-STD-3001 Vol. 1 (Crew Health) | Medical monitoring, permissible exposure, medical care levels |
| Human factors | NASA-STD-3001 Vol. 2 (Human Factors, Habitability, Environmental Health) | Displays, legibility, caution & warning, alarms, acoustics, controls |
| ISS displays | SSP 50313 Display and Graphics Commonality Standard (where ISS display rules apply) | Layout, colours, terminology consistency with ISS software |
| Software engineering | NPR 7150.2 (NASA Software Engineering Requirements) | Software classification, life cycle, documentation, reviews |
| Software assurance | NASA-STD-8739.8 (Software Assurance and Software Safety) | Hazard analysis, safety-critical software requirements, IV&V readiness |
| Cyber security | NASA-STD-1006 (Space System Protection) | Protection against unauthorised access, integrity of commands/data |
| Medical device software | IEC 62304 | Software life cycle, safety class (target class B; alarm core treated as class C) |
| Risk management | ISO 14971 | Hazard log, risk controls, residual risk |
| Usability | IEC 62366-1 | Use specification, use-related risk analysis, summative evaluation |
| Alarm systems | IEC 60601-1-8 | Alarm priorities, signals, latching, silencing, alarm logs |
| Health software security | IEC 81001-5-1 | Secure development life cycle for health software |
| Clinical data | HL7 FHIR R4, LOINC, UCUM, SNOMED CT | Records, codes, units |
| Device communication | IEEE 11073-10101 (nomenclature), -10206 (ACOM / GHS), SDC family (-20701, -20702, -10207) | Device semantics and plug-and-play |
| Bluetooth | Bluetooth SIG HRS, PLXS, BPS, HTS, WSS, GLS, GHS specifications | Device decoding |
| Imaging and ECG | DICOM PS3, EDF/EDF+, HL7 aECG, SCP-ECG | Ultrasound and ECG exchange |
| Space packets | CCSDS 133.0-B-2 Space Packet Protocol | Telemetry to ground |
| Time | CCSDS 301.0-B-4 Time Code Formats | CUC / CDS timestamps |
| Files | CCSDS 727.0-B-5 CFDP | File delivery to ground |
| Telemetry definitions | XTCE (CCSDS / OMG) | Mission database for Yamcs and other ground systems |
| DTN | Bundle Protocol v7 (RFC 9171) | Delay-tolerant delivery (v1.1) |
| ESA interoperability (optional) | ECSS-E-ST-70-41C (PUS) | Packet services if an ESA ground segment is used |
| *(hardware)* | NASA-STD-6001, JSC 20793, SSP 30237, ISO 10993 | Materials, batteries, EMC, biocompatibility |

## 2. NASA host-application needs (yet2 technology need) → CAELUM
| ID | Need (paraphrased) | Satisfied by |
|---|---|---|
| N-01 | Runs on Windows 10 | Platforms: Windows 10 LTSC and Windows 11 reference builds (`01_FEATURE_LOCK.md`) |
| N-02 | Continuous Bluetooth and USB connection to peripheral health and science devices | F-ACQ-01…05 |
| N-03 | Very resource lean; no phone functions | F-SYS-12, budgets in `09_VV_PLAN.md` |
| N-04 | Simple interface for non-programmers | `07_UI_SPEC.md`, IEC 62366-1 process |
| N-05 | Device data stored and accessible on the PC file system | F-SYS-08 exports, DICOM/EDF+ file store |
| N-06 | Configuration updatable for new hardware, apps and interfaces | F-ACQ-12, F-SYS-07, F-GND-07 |
| N-07 | Fail-over to another instance on separate hardware | F-SYS-04 |
| N-08 | No internet connection required | F-SYS-06, offline-first rule, `docs/OFFLINE_BUILD.md` |
| N-09 | Meets NASA flight-software requirements (stability, security, supportability) | NPR 7150.2 / 8739.8 evidence, `09_VV_PLAN.md` |
| N-10 | Integrates vital signs, ECG, ultrasound, secure communication (multifunctional device need) | F-ACQ-02/03/06/07, F-CLN-03/04, F-GND-05 |

## 3. System requirements (selected, numbered for tracing)
### Crew health and clinical
| ID | Requirement |
|---|---|
| R-CLN-01 | The system shall display each crew member's current vital signs with source device, measurement time and quality. |
| R-CLN-02 | The system shall never display a physiological value that was not measured, entered or imported. |
| R-CLN-03 | The system shall support the onboard medical officer and ground flight surgeon in reviewing ECG, imaging and trends. |
| R-CLN-04 | Clinical content (thresholds, protocols) shall be versioned, signed and approved before use. |
### Alarms (IEC 60601-1-8, NASA-STD-3001 Vol. 2 caution & warning principles)
| ID | Requirement |
|---|---|
| R-ALM-01 | High-priority physiological alarms shall reach the affected astronaut and CMO within 1 s of meeting their persistence condition. |
| R-ALM-02 | Alarm priority shall be conveyed by shape, text and sound, not colour alone. |
| R-ALM-03 | Unacknowledged high-priority alarms shall escalate to the whole crew and to ground on defined timers; the ground stage shall be sent on the OPS channel via path B. |
| R-ALM-04 | Technical alarms (sensor, device, data, storage) shall be distinguishable from physiological alarms. |
| R-ALM-05 | Alarm silencing shall be time-limited and logged; latched alarms remain until acknowledged. |
| R-ALM-06 | Baselines or models shall never suppress, delay or downgrade an absolute-limit alarm. |
### Security and integrity (NASA-STD-1006, IEC 81001-5-1)
| ID | Requirement |
|---|---|
| R-SEC-01 | All data at rest shall be encrypted; keys shall not be stored in plain files. |
| R-SEC-02 | All users and machine identities shall authenticate; access shall be limited by role and crew scope. |
| R-SEC-03 | Clinical and configuration actions shall be recorded in a signed, tamper-evident audit log. |
| R-SEC-04 | The software shall have no command path to vehicle systems or therapeutic devices. |
| R-SEC-05 | Uplinked items shall be accepted only with a valid signature from an authorised ground key. |
| R-SEC-06 | Data leaving the vehicle shall be filtered by per-channel allow-lists; operations channels carry no physiology (no measured value, parameter name, rule id or free text). |
| R-SEC-07 | Writes to devices shall be limited to entries in the reviewed write-exception register (`docs/write-exceptions.md`): Bluetooth SIG control writes needed for acquisition, serial poll requests defined in a device ICD, and the wristband alert through the Bluetooth Immediate Alert Service. |
### Availability and operations
| ID | Requirement |
|---|---|
| R-OPS-01 | All safety functions shall work with no ground link for at least 30 days. |
| R-OPS-02 | A standby instance shall take over within 10 s of primary failure without losing acknowledged data. |
| R-OPS-03 | The software shall install, run and update with no internet access; release builds shall be reproducible offline from a prepared, checksummed tool cache. |
| R-OPS-04 | Device time offsets shall be measured and corrected; residual offset > 50 ms shall raise a technical alarm. |
| R-OPS-05 | UTC shall be derived from TAI with a bundled IERS leap-second table updated only by signed configuration; CCSDS CUC time shall use the CCSDS epoch 1958-01-01 TAI; the MET epoch shall come from signed mission configuration. |
### Interoperability
| ID | Requirement |
|---|---|
| R-INT-01 | Telemetry to ground shall use CCSDS Space Packets with an XTCE definition. |
| R-INT-02 | Files to ground shall use CFDP; clinical records shall be exportable as FHIR R4. |
| R-INT-03 | Devices using standard Bluetooth health profiles or GHS shall work without custom code. |
| R-INT-04 | The system shall read data from cFS-based vehicle software read-only through the Software Bus; CAELUM status telemetry shall enter the vehicle as defined in `docs/icd/ICD-VEH.md` (until agreed: CCSDS packets to a configurable UDP endpoint). |
| R-INT-05 | The system shall write clinical records to, and read device observations from, the onboard medical data system using HL7 FHIR R4. |
| R-INT-06 | Delivery of high-priority alarms to the crew and to ground shall not depend on the onboard medical data system being available. |
| R-INT-07 | If the onboard medical data system is unavailable, records shall be kept locally and delivered later or sent directly by CFDP, with no data loss. |

## 4. Software classification (to be confirmed by NASA)
Proposed: **NPR 7150.2 Class C** for the application overall, with the alarm core
(`caelum-alarms`, `caelum-ingest`) developed to **safety-critical** practices: exhaustive
table-driven and property tests for every rule; coverage gates on stable Rust (`caelum-alarms`
≥ 95 % region coverage, other core crates ≥ 85 % line coverage); a nightly-toolchain job that
reports branch coverage for `caelum-alarms` (informational); and a documented independent review
of `caelum-alarms`. MC/DC coverage is not measured because Rust has no supported tooling for it
(CR-002 D3). IEC 62304 software safety class **B**, with the alarm core treated as **class C**.
These choices drive the evidence in `09_VV_PLAN.md`.

## 5. Authentic NASA Spaceflight & Environmental Data Traceability (Zero Synthetic Data)

In strict accordance with **R-CLN-02** (*"The system shall never display a physiological value that was not measured, entered or imported"*), CAELUM strictly enforces a **Zero Synthetic Data Policy**. All synthetic dataset generation scripts have been permanently excised from the repository. All physiological training sets, spaceflight analogs, environmental calibrations, and space weather monitors utilize 100% authentic data downloaded from official NASA and NOAA archives:

| NASA / Federal Data Source | Dataset Identifier & URL | Study / Instrument Context | CAELUM Subsystems & Requirements Satisfied |
|---|---|---|---|
| **NASA OSDR** (Open Science Data Repository) | **OSD-679** (LSDS-81)<br>`https://nasa-osdr.s3.amazonaws.com/` | Long-duration head-down tilt bed rest spaceflight analog modeling cephalic fluid shift, microgravity ocular syndrome (SANS), and vascular perfusion. | **M9 Personal Baseline Anomaly** (detects microgravity cephalic fluid shift deconditioning on Day 2 with 0 false notices on 60 days of real normotensive baseline); **M6 Continuous BP Trend** (vascular perfusion). Satisfies **R-CLN-01**, **R-CLN-02**, and **NASA-STD-3001 Vol. 1**. |
| **NASA OSDR** (Open Science Data Repository) | **OSD-137** (LSDS-28)<br>`https://nasa-osdr.s3.amazonaws.com/` | Spaceflight metabolic, cellular adaptation, and histology records. | Biomarker and spaceflight metabolic priors in `caelum-signal::baseline::nasa_reference`. Satisfies **NASA-STD-3001 Vol. 1**. |
| **NASA Langley ASDC** (Atmospheric Science Data Center) | **POWER API**<br>`https://power.larc.nasa.gov/api/` | 168 hours of hourly atmospheric surface pressure (`PS`), ambient temperature (`T2M`), and relative humidity (`RH2M`) at NASA Johnson Space Center (29.56°N, -95.09°W). | **M1 Signal Quality Index** (environmental channel noise stability) and `caelum-vehicle` cabin atmosphere evaluation. Satisfies **F-ENV-01** and **NASA-STD-3001 Vol. 2**. |
| **NOAA SWPC / NASA SRAG** (Space Radiation Analysis Group) | **GOES-18 Integral Protons**<br>`https://services.swpc.noaa.gov/json/goes/` | Live satellite integral proton flux ($\ge 1\text{ MeV}$, $\ge 10\text{ MeV}$, $\ge 100\text{ MeV}$) space radiation observations. | `caelum-vehicle::spe` Solar Particle Event (SPE) alert classification state machine (`none`, `watch`, `warning`, `in_progress`). Satisfies **F-ENV-02** and **NASA-STD-3001 Vol. 2 §8**. |
