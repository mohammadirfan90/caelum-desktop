# 05. Data model

## 1. Parameter catalog (`caelum-domain::parameters`)
Each parameter: key, label, canonical unit (UCUM), value type (numeric or enum), category
(vital-signs, derived, environment, context, technical), plausible range (sensor-fault limits, not
clinical), `stale_after_s` (age after which the latest value is STALE), LOINC code (if any;
verified against the LOINC table before use, LOINC version recorded), IEEE 11073-10101 code (if
any), XTCE type, downlink channels allowed, retention class. No parameter has a default value: a
parameter with no valid reading has no value.
v1.0 set (minimum): heart_rate, pulse_rate, rr_interval, hrv_rmssd, hrv_sdnn, spo2, respiration_rate,
respiration_amplitude, core_temp, body_temp_measured, skin_temp_proximal, skin_temp_distal,
bp_systolic, bp_diastolic, bp_trend_systolic, pat, eda, activity_level, sleep_stage,
af_probability, rhythm_class, sqi_ecg, sqi_ppg, sqi_resp, weight, glucose, pvt_median_rt,
checkin_mood, checkin_symptoms, cabin_co2, cabin_o2, cabin_pressure, cabin_temp, cabin_humidity,
radiation_dose_rate, radiation_dose_cumulative, spe_state, exercise_load, exercise_duration,
battery_level, device_clock_offset, rssi, packet_loss_pct, reconnect_count.

Notes (CR-002):
- `core_temp` is the F-SIG-05 estimate only. `body_temp_measured` is a thermometer reading with
  its site code (Bluetooth HTS Temperature Type; `unknown` when the device sends none).
  Thermometer readings never go to `core_temp`.
- `pulse_rate` uses LOINC 8889-8 (heart rate by pulse oximetry), distinct from `heart_rate`
  (8867-4). Both are verified against the LOINC table like every other code.
- `spe_state` is an enum; its values come from `docs/icd/ICD-VEH.md` (open item OI-2).
- `rssi` (dBm), `packet_loss_pct` (%) and `reconnect_count` (count) are technical link metrics per
  device (F-ACQ-10).
- Missing context (for example no current `activity_level`) is `unknown`, never a default (SR-018).

## 2. Core tables (SQLCipher)
Database encryption: SQLCipher 4 (AES-256-CBC with HMAC-SHA512 page authentication), key from the
OS keystore. Attachment file store: AES-256-GCM.

| Table | Key fields |
|---|---|
| crew_member | id, name, role, mission, active, photo hash |
| user | id, role, crew_member_id?, credential (argon2), key id, disabled |
| device | id, adapter, model, vendor, serial, firmware, accuracy_class, calibration_due, crew_member_id, status, last_seen, clock_offset_ms |
| observation | id, crew_member_id, device_id, parameter, value, unit, measured_at (TAI ns), received_at, quality, classification (measured/derived/entered/imported), algorithm_version?, context JSON |
| waveform_chunk | id, crew_member_id, device_id, signal, start_tai, sample_rate, encoding, data (compressed), quality |
| baseline | crew_member_id, parameter, median, mad, n, window, status, method_version |
| alarm | id, crew_member_id?, kind (physiological/technical), category (CARDIO/RESPIRATORY/THERMAL/ENVIRONMENT/TECHNICAL), rule_id, ruleset_version, priority, state, latched, trigger_value, why JSON, action, created_at, acknowledged_by/at, silenced_until, resolved_at, escalation_stage |
| alarm_notification | alarm_id, audience, stage, at, delivered_to |
| encounter | id, crew_member_id, author, SOAP fields, signed_at, attachments |
| attachment | id, kind (dicom/ecg/file), content_hash, metadata JSON |
| checklist_run | id, protocol_id, protocol_version, alarm_id?, steps JSON, started/completed |
| checkin | id, crew_member_id, answers JSON, privacy_tier (Private / Medical / Status) |
| model_output | id, model_id, model_hash, crew_member_id, output JSON, explanation JSON, at |
| downlink_item | id, seq, channel, priority, payload (signed), status, created_at, sent_at, ack_at |
| uplink_item | id, kind, signature, verified_by, applied_at |
| audit_entry | id, at, actor, role, action, target, details JSON, prev_hash, hash, signature |
| config_bundle | id, kind, version, signature, applied_at |
| mds_outbox | id, resource_type, client_id (idempotency), payload, state (pending/sent/acked/failed), attempts, last_error, created_at, acked_at |
| mds_inbox | id, source_id, resource_type, received_at, ingested (bool), dedup_key |

## 3. Provenance and simulation guard
`Observation.classification` is an enum with no "simulated" variant in release builds. In
`test-equipment` builds an extra variant exists, and export/downlink functions take a type that
cannot be built from it, so test data cannot leave the test environment.
Readings a device itself flags as demonstration or test data (for example PLXS Measurement Status
bits 10 and 11) are rejected in release builds (SR-017); only `test-equipment` builds may ingest
them, and then only with the simulated classification.

## 4. Time
All timestamps stored as TAI nanoseconds plus the source clock; UI shows UTC and MET. Device clock
offset tracked per device; corrections recorded in provenance. UTC is derived from TAI with a
bundled IERS leap-second table that only signed configuration can update. CCSDS CUC timestamps use
the CCSDS epoch 1958-01-01 TAI. The MET epoch comes from signed mission configuration (R-OPS-05).

## 5. Retention and export
Configurable per retention class. Exports: FHIR R4 Bundle, CSV (per parameter), EDF+ (waveforms),
DICOM (originals), de-identified research package (date-shifted, IDs pseudonymised, consent-checked).
