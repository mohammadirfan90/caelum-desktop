# 00. Analysis

## 1. Goal, stated precisely
"Perfect" and "world's greatest" are not testable. For this project they mean the following
measurable bar, which every milestone gate checks:

| Quality goal | Measurable meaning |
|---|---|
| Never misleads | 0 fabricated values in any build; every number on screen has device, time and quality |
| Never misses an emergency it is designed to catch | 100 % detection on the alarm test corpus; alarm latency ≤ 1 s after persistence is met |
| Does not cry wolf | False-alarm rate measured on recorded and analog data; target < 1 non-actionable alarm per crew-week |
| Works with no Earth | Every safety function operates with the ground link down for 30 days |
| Survives failure | Standby laptop takes over within 10 s; no acknowledged data lost |
| Trustworthy to agencies | Requirements traced to NASA/IEC standards; tests, hazard log and SBOM for every release |

**Honest limits.** Software alone cannot become flight-certified. NASA classifies, reviews and
qualifies flight software and hardware through its own processes (NPR 7150.2, safety review
panels, flight-surgeon approval of clinical thresholds). This plan builds the product so that it
is *ready* for that process: built to the standards, with the evidence those reviews ask for.
"No demo" is honoured: release builds contain no simulated data path. Device emulators exist only
as **test equipment** for automated and hardware-in-the-loop testing, compiled out of releases.

## 2. Problem being solved
1. On long missions astronauts must spot health changes themselves, with delayed or no ground contact.
2. Onboard medical devices are discrete and poorly integrated; data moves by manual file transfer.
3. NASA asked for an offline Windows application that hosts Bluetooth and USB medical and
   scientific devices in place of phones: lean, simple to use, updatable for new devices, able to
   fail over to a second instance, and ultimately meeting flight-software requirements.
4. Orion's digital medical devices depend on mobile apps while the vehicle runs Windows.
5. Ground needs timely, trustworthy reports without raw private data leaving the vehicle unnecessarily.

## 3. What carries over from the prototype (`caelum-next` v3.1)
| Asset | Carry over as | Notes |
|---|---|---|
| Design tokens (mint canvas, ink, signal green, warning amber, dark theme) | `ui/src/styles/tokens.css`, same look with the CR-002 accessibility fixes | See `07_UI_SPEC.md` §1 |
| Shape-coded levels (circle, diamond, triangle, octagon) | UI primitive `LevelGlyph` | Keeps NASA-STD-3001 "not colour alone" principle |
| Glove mode, text size, reduced motion | Accessibility settings | Extended to 200 % text and kiosk mode |
| Screens: Home, Body, Alerts, Checks, Environment, Radiation, Baseline, Privacy, Sources, Mission Control | Reworked for desktop multi-pane layout | Module list locked in `01_FEATURE_LOCK.md` |
| Alert rules, persistence, hysteresis, escalation, supersession | Port to Rust `caelum-alarms` | Python tests become the Rust acceptance corpus |
| Parameter catalog (LOINC, IEEE 11073, ranges) | `caelum-domain::parameters` | Extended per `05_DATA_MODEL.md` |
| BLE decoders (HR, PLX, BP, thermometer), FHIR import/export, CCSDS framing, Open MCT dictionary | Port to Rust crates | Replace hand-rolled CCSDS with `spacepackets` |
| HMAC audit chain, downlink allow-lists, role scoping | Port, upgrade audit to Ed25519 signatures | |

## 4. What must change
| Prototype | Desktop product |
|---|---|
| FastAPI + Next.js, two servers | One Tauri app; Rust core in-process; optional LAN service for other crew laptops |
| SQLite unencrypted | SQLCipher encrypted at rest, key in OS keystore, crash-safe WAL |
| Simulator and demo modes | Removed. Test equipment only, feature-gated, never in releases |
| Many screens on demo data | Every v1.0 screen backed by real data paths; modules without data are not shipped |
| Bearer tokens in browser storage | OS-level session, role login, auto-lock, re-auth for sensitive actions |
| Thresholds in code | Signed TOML ruleset, versioned, approved, loaded and verified at start |
| Web Bluetooth not used; BLE via HTTP | Native BLE (`btleplug`) and USB/serial in the core, continuous background connection |
| No fail-over | Primary/standby pair with replicated, signed event log |
| No MCC integration beyond JSON | CCSDS packets, XTCE dictionary, Yamcs link, Open MCT plugin, CFDP files, ICD |

## 5. Key risks
| Risk | Mitigation |
|---|---|
| Clinical thresholds wrong | Ruleset is data, signed, requires recorded approver; flight-surgeon review gate (M13) |
| Device vendor protocols closed | v1.0 uses open standards (Bluetooth SIG profiles, GHS, DICOM, EDF+, HL7 aECG, FHIR); vendor adapters only via plug-in SDK |
| IEEE 11073 SDC has no mature Rust stack | Deferred to v1.1 as an isolated adapter process (locked decision) |
| WebView differences across OS | Windows (WebView2) is the reference platform; Linux and macOS tested in CI |
| Scope creep | Feature lock with change control; Claude Code instructed to refuse unlisted features |
| Claude Code drift over long builds | Milestone gates, requirement IDs in tests, `CLAUDE.md` rules, small PR-sized tasks |
