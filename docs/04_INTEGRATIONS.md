# 04. Integrations

Every interface gets an Interface Control Document (ICD) from `11_ICD_TEMPLATE.md` before it is
built. Real NASA mission-control and vehicle interfaces are defined by NASA ICDs; CAELUM builds
to the open standards those systems use and adapts to the ICD in configuration, not code.

## 1. Medical and wearable devices (inbound only)
| Interface | Standard / transport | Crate / approach | Data | Feature |
|---|---|---|---|---|
| Standard BLE health devices | Bluetooth SIG HRS 0x2A37, PLXS 0x2A5E/0x2A5F, BPS 0x2A35, HTS 0x2A1C, WSS, GLS | `btleplug`; decoders ported from prototype + new | HR, RR intervals, SpO₂, PR, BP, temperature, weight, glucose | F-ACQ-01/02 |
| Generic Health Sensor | Bluetooth GHS (IEEE 11073-10206 ACOM, -10101 codes) | `btleplug` + ACOM decoder | Any observation incl. ECG sample arrays | F-ACQ-03 |
| CAELUM sensor kit | BLE custom service (documented in its ICD), 1 Hz features + on-demand raw | `btleplug` | ECG, RIP ×2, IMU, temps, PPG, EDA, battery, SQI | F-ACQ-05 |
| USB / serial devices | CDC-ACM, HID; vendor protocols via decoders | `serialport`, `hidapi` | Device-specific | F-ACQ-04 |
| Ultrasound / imaging | DICOM files, folder watch, C-STORE SCP | `dicom` (dicom-rs) | Images, cine loops, metadata | F-ACQ-06 |
| ECG recordings | EDF/EDF+, HL7 aECG (XML), DICOM waveform, SCP-ECG | own parsers + `quick-xml` + `dicom` | 12-lead / Holter | F-ACQ-07 |
| Medical records | HL7 FHIR R4 JSON | `serde` models | Observations, devices, reports | F-ACQ-08 |
| Monitors / defibrillators | IEEE 11073 SDC (read-only consumer) | isolated adapter process | Waveforms, numerics, device alarms | v1.1 |
| Smart garments (Astroskin-class) | Vendor export / API | plug-in SDK | Garment signals | F-ACQ-13 |

Rules: adapters are read-only (no write characteristics except those required for standard
pairing, notifications and record-access control points defined by the SIG profiles); every
reading carries device id, device time, received time and quality; unknown devices are listed but
not trusted until registered by a CMO or administrator.

## 2. Vehicle (read-only data in, status telemetry out)
| Interface | Standard / transport | Notes | Feature |
|---|---|---|---|
| cFS Software Bus | cFS message format (CCSDS primary + cFS secondary header) over Software Bus Network (SBN) or a configured UDP telemetry output | Subscribe to configured message IDs for CO₂, O₂, pressure, temperature, humidity, radiation, exercise devices, vehicle time | F-GND-11, F-ENV-01..03 |
| Health status publication | cFS telemetry messages with a dedicated MID range | Only telemetry MIDs on an allow-list; no command MIDs can be constructed by the code | F-GND-11 |
| Vehicle time | cFS time services / configured time source | MET and TAI/UTC correlation; drift monitoring | F-ACQ-11 |
| Dosimeters | Via vehicle bridge or direct USB/BLE adapter | Dose rate, cumulative dose, SPE flags | F-ENV-02 |

## 3. Mission control and ground
| Interface | Standard / transport | Content | Feature |
|---|---|---|---|
| Telemetry packets | CCSDS 133.0-B-2 Space Packets, CUC time secondary header, APID plan per ICD | Crew status, alarm events, vitals summaries (per channel allow-list) | F-GND-01 |
| Mission database | XTCE generated from the parameter catalog and APID plan | Lets any XTCE-capable ground system decode CAELUM packets | F-GND-02 |
| Yamcs | UDP/TCP packet link into a Yamcs TM data link; XTCE loaded as MDB | Archive, alarms, displays in mission control | F-GND-03 |
| Open MCT | Plugin package: telemetry provider (via Yamcs or CAELUM ground gateway), crew board, alert timeline, event-packet viewer | Flight-surgeon and BME consoles | F-GND-04 |
| File delivery | CCSDS CFDP class 1 and 2 (`cfdp-rs`) over the vehicle's ground link | Event packets, daily reports, DICOM, ECG, exports | F-GND-06 |
| Uplink | CFDP files with Ed25519 signatures | Rulesets, protocol library, models, config, software updates | F-GND-07 |
| Store-and-forward | Persistent queue with priorities (CRITICAL / ROUTINE / RESEARCH) and per-channel allow-lists | Works through loss of signal | F-GND-05 |
| Delay-tolerant networking | Bundle Protocol v7 adapter (to an onboard DTN node) | Deep-space operations | v1.1 |
| ESA ground segments | PUS (ECSS-E-ST-70-41C) packet services via `spacepackets` | If required by an ESA ICD | config option |

Routing: the `OPS` channel always uses path B (direct cFS). The `MEDICAL` channel uses path A
(medical data system) or A' (direct CFDP) per mission configuration.

Channels (allow-lists enforced in `caelum-ground`):
- `OPS` (mission control): crew status level, alarm state, technical health. No raw physiology.
- `MEDICAL` (flight surgeon, BME): full clinical detail, encrypted payloads, private medical data.
- `RESEARCH`: de-identified, consented datasets only.

## 3a. Onboard medical data system (MDA-class) — F-GND-12/13/14
| Aspect | Design |
|---|---|
| Role | System of record for the crew health record onboard, when present. CAELUM is the real-time monitoring and alarm layer feeding it. |
| Protocol | HL7 FHIR R4 REST (configurable base URL on the crew LAN), mutual TLS, machine identity |
| CAELUM writes | Observation (summaries per configurable interval, all alarm-related values), DiagnosticReport (alarm events, ECG reviews), Encounter, Media / DocumentReference (DICOM, ECG files), Device, Provenance |
| CAELUM reads | Observations and devices the medical system collects directly, previous encounters, problem list, medications; ingested with SR-016 de-duplication |
| Sync | Outbox per resource with idempotent client ids, retry with back-off, conflict policy (medical system wins for records it authored; CAELUM wins for its own observations) |
| Health | Connection state shown in the status footer; technical alarm if unreachable > configurable time |
| Fallback | Local record continues; per mission, either hold until reconnect or hand files to CFDP directly (path A') |
| Data policy | Summary-first (F-GND-14): no continuous raw waveforms unless linked to an event or requested |
| ICD | `docs/icd/ICD-MDS.md` (draft) to be agreed with the medical-system owner |

## 4. Ground console profile
CAELUM runs in `ground` profile on flight-surgeon and BME workstations: it ingests downlinked
packets and files, rebuilds the crew record, provides the same clinical screens read-only plus
ground-only tools (event-packet review, private medical conference notes, uplink preparation and
signing workflow). This is the "mission control computer" side of CAELUM; it also feeds Yamcs and
Open MCT for the wider control room.

## 5. Medical record systems
FHIR R4 export (Bundle of Patient, Observation, Device, DiagnosticReport, Media, Encounter,
Provenance) for NASA's medical data systems (e.g. the Medical Data Architecture) or any FHIR
server. Codes: LOINC for measurements, UCUM units, SNOMED CT for problems, IEEE 11073 for device
metrics. Simulated or test-equipment data can never be exported (blocked by type).

## 6. Crew LAN
Other crew laptops/tablets connect to the primary as secondary stations (mutual TLS, role login),
receive the event stream and render the same UI. Used for the crew-wide emergency view.
