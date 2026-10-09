# 05. Data model

## 1. Parameter catalog (`caelum-domain::parameters`)
Each parameter: key, label, canonical unit (UCUM), category (vital-signs, derived, environment,
context, technical), plausible range (sensor-fault limits, not clinical), LOINC code (if any),
IEEE 11073-10101 code (if any), XTCE type, downlink channels allowed, retention class.
v1.0 set (minimum): heart_rate, pulse_rate, rr_interval, hrv_rmssd, hrv_sdnn, spo2, respiration_rate,
respiration_amplitude, core_temp, skin_temp_proximal, skin_temp_distal, bp_systolic, bp_diastolic,
bp_trend_systolic, pat, eda, activity_level, sleep_stage, af_probability, rhythm_class, sqi_ecg,
sqi_ppg, sqi_resp, weight, glucose, pvt_median_rt, checkin_mood, checkin_symptoms, cabin_co2,
cabin_o2, cabin_pressure, cabin_temp, cabin_humidity, radiation_dose_rate, radiation_dose_cumulative,
exercise_load, exercise_duration, battery_level, device_clock_offset.

## 2. Core tables (SQLCipher)
| Table | Key fields |
|---|---|
| crew_member | id, name, role, mission, active, photo hash |
| user | id, role, crew_member_id?, credential (argon2), key id, disabled |
| device | id, adapter, model, vendor, serial, firmware, accuracy_class, calibration_due, crew_member_id, status, last_seen, clock_offset_ms |
| observation | id, crew_member_id, device_id, parameter, value, unit, measured_at (TAI ns), received_at, quality, classification (measured/derived/entered/imported), algorithm_version?, context JSON |
| waveform_chunk | id, crew_member_id, device_id, signal, start_tai, sample_rate, encoding, data (compressed), quality |
| baseline | crew_member_id, parameter, median, mad, n, window, status, method_version |
| alarm | id, crew_member_id?, kind (physiological/technical), rule_id, ruleset_version, priority, state, latched, trigger_value, why JSON, action, created_at, acknowledged_by/at, silenced_until, resolved_at, escalation_stage |
| alarm_notification | alarm_id, audience, stage, at, delivered_to |
| encounter | id, crew_member_id, author, SOAP fields, signed_at, attachments |
| attachment | id, kind (dicom/ecg/file), content_hash, metadata JSON |
| checklist_run | id, protocol_id, protocol_version, alarm_id?, steps JSON, started/completed |
| checkin | id, crew_member_id, answers JSON, privacy_tier |
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

## 4. Time
All timestamps stored as TAI nanoseconds plus the source clock; UI shows UTC and MET. Device clock
offset tracked per device; corrections recorded in provenance.

## 5. Retention and export
Configurable per retention class. Exports: FHIR R4 Bundle, CSV (per parameter), EDF+ (waveforms),
DICOM (originals), de-identified research package (date-shifted, IDs pseudonymised, consent-checked).
