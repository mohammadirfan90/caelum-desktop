# 03. Architecture

## 1. Process model and onboard data flow
```
 Hardware (wearables, medical devices) ──┐          ┌── Vehicle data via cFS (read-only):
                                         ▼          ▼    CO₂, O₂, pressure, radiation, exercise, time
              ┌──────────────── CAELUM (Tauri 2 app, Rust core) ────────────────┐
              │ adapters → ingest → signal → alarm engine → escalation → notifier │──► crew stations (always
              │ encrypted local record + store-and-forward queue + audit log      │    sound), wristband alert
              └──────────┬─────────────────────────────────────────┬────────────┘
   (A) medical-record path│ FHIR R4 read/write, attachments          │(B) operational path
                          ▼                                          ▼ CCSDS TM packets → cFS (ICD-VEH)
        Onboard medical data system (MDA-class)              Vehicle downlink (CCSDS / DTN)
                          │ sync to ground                           │
                          ▼                                          ▼
        Ground medical record ◄── fallback (A'): CAELUM sends     Yamcs / agency MCS
        and research archive        files by CFDP itself          → Open MCT consoles
```
- **Path A (medical record):** complete clinical data. Primary route is the onboard medical data
  system (two-way). If it is absent or unreachable, CAELUM keeps everything locally and either
  waits or sends directly by CFDP (configurable per mission).
- **Path B (operational):** small, high-priority status and alarm telemetry with no physiology
  (`04` §3), built by `caelum-ground` as telemetry-only CCSDS packets and sent directly to the
  vehicle. Never routed through the medical system (SR-015). The ground escalation stage of every
  alarm uses this path. How the packets enter cFS (SBN, CI_LAB or a ground-link gateway) is open
  item OI-1 in `docs/icd/ICD-VEH.md`; until it is agreed, path B emits to a configurable UDP
  endpoint.
- Alerts to the crew are always local and immediate; neither path is in the alarm loop. Crew
  stations always sound; the wristband alert is an addition, never the only signal.

- One OS process. The Rust core owns all data and logic; the UI holds no authority.
- Optional isolated adapter processes (plug-in SDK, later SDC) talk to the core over a local,
  authenticated socket. A crashing adapter never takes down the core.
- Supervisors restart failed tasks; a watchdog raises technical alarms for stuck tasks.

## 2. Cargo workspace
```
crates/
  caelum-domain     parameters, units, codes, IDs, time types (MET/TAI/UTC), errors      [no I/O]
  caelum-store      SQLCipher, migrations, repositories, retention, export
  caelum-ingest     validation, unit conversion, plausibility, provenance, time correction
  caelum-signal     ECG/QRS/HRV, SQI, respiration, SpO₂, core-temp, activity
  caelum-alarms     ruleset loader + signature check, rule engine, escalation, technical alarms
  caelum-ml         ONNX runtime wrapper, model registry, feature builders, explanations
  caelum-devices    adapter trait, registry, BLE (btleplug), pairing modules (WinRT, BlueZ via
                    bluer, macOS system pairing), USB/serial, GHS, SIG profiles, sensor-kit
                    protocol, DICOM (dicom-rs), ECG files, FHIR import
  caelum-vehicle    cFS Software Bus bridge, subscribe only (no write API), environment &
                    radiation normalisation
  caelum-ground     CCSDS packets (spacepackets), CFDP (cfdp-rs), XTCE generator, Yamcs link,
                    downlink queue, uplink verifier, FHIR export, routing (paths A / A' / B),
                    path B status publisher (telemetry-only packet type)
  caelum-mds        onboard medical data system connector: FHIR client, sync state machine,
                    attachment hand-off, read-back of device observations, health monitor
  caelum-security   users, roles, sessions, argon2, keystore, Ed25519 signing, audit chain
  caelum-cluster    primary/standby replication, heartbeat, takeover, LAN service (axum)
  caelum-core       wiring: task supervisor, event bus, configuration, self-test
  caelum-test-equipment  device emulators, scenario player, fault injection  [feature-gated]
src-tauri/          Tauri app: commands (thin), state, windows, tray, permissions
ui/                 React app, design system, screens
plugins/openmct/    Open MCT plugin package (TypeScript)
tools/              ruleset signer, config signer, XTCE export CLI, data export CLI
```
Dependency rule: `domain` ← everything; no crate depends on `src-tauri`. `vehicle` has no
write API. `devices` writes only through calls listed in `docs/write-exceptions.md`, each marked
in code with its register ID. A CI script fails on any unregistered write call in either crate.

## 3. Data flow and timing
1. Adapter receives bytes → decoder → `RawReading` (device time, payload, quality hints).
2. Ingest: time correction (device offset), unit conversion, plausibility, provenance → `Observation`.
3. Store: append to WAL-backed tables; high-rate waveforms go to chunked binary blobs (1 s chunks).
4. Signal processing produces derived observations (HR from ECG, SpO₂ from PPG...), each tagged
   `classification = derived` with algorithm version.
5. Alarm engine evaluates every observation synchronously in the ingest path (no queue between
   data and safety logic); escalation timer runs at 4 Hz. Absence-of-event rules (e.g. asystole)
   are also checked on that 4 Hz tick, so they fire even when no new observation arrives.
6. Event bus fans out to UI (IPC events), LAN stations, downlink queue and audit log.
Latency budget: device sample → UI ≤ 500 ms; persistence met → alarm on all crew stations ≤ 1 s.

## 4. Storage
- SQLCipher 4 database per deployment (AES-256-CBC with HMAC-SHA512 page authentication); 256-bit
  key from OS keystore (Windows DPAPI / Credential Manager, macOS Keychain, Linux Secret Service)
  or an external hardware token.
- Waveforms: compressed chunks (lossless) in a separate encrypted table; DICOM and ECG files in an
  encrypted (AES-256-GCM, `aes-gcm`) content-addressed file store (SHA-256 names).
- Retention policy configurable; defaults: raw waveforms 14 days onboard (after confirmed
  downlink or export), derived observations for mission duration, audit log never deleted.

## 5. Security
- Login per role; crew members can use PIN + device-bound key for speed; ground uses password + key.
- Sessions auto-lock (configurable, default 10 min onboard); sensitive actions require re-auth.
- Break-glass: any crew member can open the emergency view and protocols without login; the
  action is audited and notified.
- Audit: append-only, hash-chained, each entry signed with the instance Ed25519 key.
- Uplink verification: Ed25519 signatures from configured ground keys; ruleset and models also
  carry approver signatures.
- Ruleset keys: development signing keys are accepted only by development builds, which then show
  a "DEVELOPMENT RULESET" banner; release builds accept only flight keys with a medical approver
  signature (SR-009).
- Tauri capabilities: minimal allow-list; no shell, no arbitrary file system access from UI;
  CSP locked; no remote content.

## 6. Fail-over (F-SYS-04)
- Two onboard laptops: `primary` and `standby`. Primary streams its signed event log to standby
  over the crew LAN (mutual TLS). Standby applies it and stays read-only.
- Heartbeat every 1 s; standby promotes after 5 missed heartbeats (≤ 10 s total including device
  reconnection). Fencing: promoted standby increments an epoch; the old primary, if it returns,
  sees a higher epoch and demotes itself.
- Devices reconnect to whichever instance is primary (BLE bonds shared through the replicated
  device registry where the OS allows; otherwise re-bond procedure in the protocol library).

## 7. Configuration and updates
- Signed configuration bundles: crew, devices, channels, APIDs, ruleset, protocol library, models.
- Updates are signed installer packages delivered by uplink (CFDP) or removable media; the
  updater verifies signatures and keeps the previous version for rollback.
- Release builds are reproducible offline from a prepared, checksummed tool cache
  (`docs/OFFLINE_BUILD.md`). Windows installers embed the WebView2 offline installer
  (`offlineInstaller`); flight builds may use `fixedRuntime` to pin the WebView2 version.
