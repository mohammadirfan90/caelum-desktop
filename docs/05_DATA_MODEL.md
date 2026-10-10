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
  Thermometer readings never go to `core_temp`. Sites are classed core-equivalent or peripheral
  for the temperature rules (`06` §3).
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
| device | id, adapter, model, vendor, serial, firmware, accuracy_class, calibration_due, crew_member_id, device_user? (user slot on multi-user devices, CR-008), status, last_seen, clock_offset_ms |
| observation | id, crew_member_id, device_id, parameter, value, unit, measured_at (TAI ns), received_at, quality, classification (measured/derived/entered/imported), measurement_mode (continuous/spot, from the device profile), algorithm_version?, context JSON |
| waveform_chunk | id, crew_member_id, device_id, signal, start_tai, sample_rate, encoding, data (compressed), quality |
| baseline | crew_member_id, parameter, median, mad, n, window, status, method_version |
| alarm | id, crew_member_id?, kind (physiological/technical), category (CARDIO/RESPIRATORY/THERMAL/ENVIRONMENT/TECHNICAL), rule_id, ruleset_version, priority, state (incl. CONFIRMING for spot readings, SR-019), confirming_readings JSON?, latched, trigger_value, why JSON, action, created_at, acknowledged_by/at, ground_acknowledged_by/at (ground stage only, ROLE_MATRIX §5.4), silenced_until, resolved_at, escalation_stage, title (ruleset title, physiological alarms; CR-012) |
| alarm_notification | alarm_id, audience, stage, at, delivered_to |
| encounter | id, crew_member_id, author, SOAP fields, vitals snapshot (CR-019), created_at, signed_at (immutable after), attachments |
| attachment | id, kind (dicom/ecg/file), content_hash, metadata JSON, crew_member_id, filename, created_at (file: AES-256-GCM, CR-019 §6) |
| checklist_run | id, protocol_id, protocol_version, alarm_id?, crew_member_id?, started_by, steps JSON, started/completed |
| medication_entry | id, crew_member_id (doses only), kind (dose/kit_use/kit_restock), item, amount, unit, note, by_user, at (F-CLN-08, CR-019 §5) |
| checkin | id, crew_member_id, answers JSON, privacy_tier (Private / Medical / Status) |
| model_output | id, model_id, model_hash, crew_member_id, output JSON, explanation JSON, at |
| downlink_item | id, seq, channel, priority, payload (signed), status, created_at, sent_at, ack_at |
| uplink_item | id, kind, signature, verified_by, applied_at |
| audit_entry | id, at, actor, role, action, target, details JSON, prev_hash, hash, signature |
| config_bundle | id, kind, version, signature, applied_at, content (verified bytes, re-verified before use; SR-009 fallback) |
| mds_outbox | client_id (idempotency), record JSON: resource, kind, priority, state (pending/in flight/acknowledged/held for review), attempts, next attempt, last error, server id (CR-016) |
| mds_inbox_seen | source_id (SR-016 de-duplication), version |
| mds_state | key, value: read-back high-water mark, summary watermark, client identity (CR-016 §10) |
| cluster_log | seq, fence (epoch, node), kind, at_tai, payload, prev_hash, signature: the signed, hash-chained event log replicated to the standby (F-SYS-04, CR-017) |
| cluster_quarantine | entries a fenced primary wrote that were never acknowledged, with diverted_at; kept for review, never re-applied |
| cluster_state | key, value: highest fence seen, crew-LAN identity (certificate chain and key), capture on/off, seeded |
| cluster_outbox | seq, tbl, key: rows changed on the primary while capture is on (V9 triggers), waiting to become log entries |
| audit_replica | origin node, then the audit entry columns: the primary's audit chain held on the standby, apart from the standby's own |

## 3. Provenance and simulation guard
`Observation.classification` is an enum with no "simulated" variant in release builds. In
`test-equipment` builds an extra variant exists, and export/downlink functions take a type that
cannot be built from it, so test data cannot leave the test environment.
Readings a device itself flags as demonstration or test data (for example PLXS Measurement Status
bits 10 and 11) are rejected in release builds (SR-017); only `test-equipment` builds may ingest
them, and then only with the simulated classification.

## 4. Time
All timestamps stored as TAI nanoseconds plus the source clock; UI shows UTC and MET. Device clock
offset tracked per device (measured from request/answer exchanges, shortest round trip; residual
above 50 ms raises a technical alarm, R-OPS-04); corrections recorded in provenance. A device time
without a measured offset is not used: the reading gets the station receipt time, marked
`receipt_unsynchronised`. A corrected time more than 1 s after receipt is rejected as a clock
fault. UTC is derived from TAI with a
bundled IERS leap-second table that only signed configuration can update. CCSDS CUC timestamps use
the CCSDS epoch 1958-01-01 TAI. The MET epoch comes from signed mission configuration (R-OPS-05).

## 5. Retention and export
Configurable per retention class (CR-022): `Mission` data is never removed by retention; `Technical` parameters and raw waveform chunks are removed after the limits of the signed `retention` bundle, except within the alarm guard of the same crew member; without that bundle nothing is removed. Exports: FHIR R4 Bundle, CSV (per parameter), EDF+ (waveforms),
DICOM (originals), de-identified research package (date-shifted, IDs pseudonymised, consent-checked).
