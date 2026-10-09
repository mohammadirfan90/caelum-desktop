# 06. Safety and alarms

## 1. Principles (safety requirements)
| ID | Safety requirement |
|---|---|
| SR-001 | Absolute-limit rules are evaluated for every valid observation; nothing can disable them except a signed ruleset change. |
| SR-002 | Baselines and models can add lower-priority notices but can never suppress, delay or downgrade an absolute-limit alarm. |
| SR-003 | A rule fires only after its persistence window; it resolves only after its clear threshold holds for its clear window (hysteresis). |
| SR-004 | A data gap longer than the rule's `max_gap_s` restarts persistence timing. A value older than its parameter's `stale_after_s` is STALE and raises a technical alarm, never a "normal" state. |
| SR-005 | Observations with quality POOR/UNUSABLE pause physiological rules for that signal and raise a technical alarm. |
| SR-006 | Alarm state is per crew member and per rule; a higher-priority alarm supersedes lower ones for the same parameter. |
| SR-007 | High-priority alarms escalate on timers until acknowledged; escalation state survives restart and fail-over. |
| SR-008 | Silencing is time-limited (max 2 min high, 10 min medium), never removes the visual alarm, and is audited. |
| SR-009 | The ruleset is loaded only if its signature and approver record verify; otherwise the app starts in a safe state that shows a technical alarm and uses the last verified ruleset. Development signing keys are accepted only by development builds, which then show a "DEVELOPMENT RULESET" banner; release builds accept only flight keys with a medical approver signature. |
| SR-010 | Every alarm records why it fired: rule id, version, values, durations, device and quality. |
| SR-011 | No code path sends commands to vehicle systems or therapeutic devices. `caelum-vehicle` has no write API; the path B publisher can only build telemetry packets; device writes are limited to entries in `docs/write-exceptions.md` (R-SEC-07), checked by CI. |
| SR-012 | Clock faults (offset > 50 ms or time going backwards) raise a technical alarm and are corrected in provenance. |
| SR-013 | Alarm sounds and visuals meet NASA-STD-3001 Vol. 2 / IEC 60601-1-8 distinguishability; tested in usability evaluation. |
| SR-014 | Break-glass access to the emergency view and protocols never requires a login. |
| SR-015 | Alarm notification to crew stations, wristbands and the operational ground path (path B, OPS channel) is independent of the onboard medical data system. Crew stations always sound; the wristband alert is in addition and never the only audible signal. |
| SR-016 | Observations read from the medical data system go through the same ingest validation and alarm evaluation as direct device data, with duplicates removed by source id. |
| SR-017 | Readings that a device flags as demonstration or test data (e.g. PLXS Measurement Status bits 10 and 11) are rejected in release builds. |
| SR-018 | Missing context is `unknown`, never a default. A rule conditioned on context (e.g. "at rest") is evaluated when the context is unknown, and the alarm's why states it (e.g. "activity unknown"). |

## 2. Priorities and presentation
| CAELUM level | IEC 60601-1-8 priority | Shape | Sound | Latching |
|---|---|---|---|---|
| Critical | High | Octagon | High-priority pattern, repeating, on crew stations; wristband Immediate Alert in addition | Latched until acknowledged |
| Warning | Medium | Triangle | Medium-priority pattern on crew stations | Latched until acknowledged |
| Observe | Low | Diamond | Single low tone (optional) | Non-latching |
| Advisory (technical) | Low / technical | Square with "!" | Technical tone | Clears when cause clears |
| Normal | — | Circle | — | — |

## 3. Initial ruleset (DRAFT, revised by CR-002; needs flight-surgeon approval before use)
Ported from the prototype's tested engine; times in seconds.

Rule types:
- **Threshold** (absolute limit): fire and clear thresholds with persistence and clear windows,
  `max_gap_s`, an optional context condition (SR-018).
- **Absence-of-event:** fires when an expected event (e.g. a QRS complex) is absent for a window
  while its gating signal is valid (good SQI). Evaluated on each gating input and on the 4 Hz tick.
  The M3 engine tests it with synthetic event streams; `asystole.critical` is wired to real QRS
  detection in M7.
- **Baseline** (observe only): robust z-score against the personal baseline (F-ALM-08).
- **Model-driven:** observe level only until the model is `VALIDATED` and a CR promotes the rule.

Every rule carries `max_gap_s`. The draft value is 30 s for every rule (the prototype's global
value) until the flight-surgeon review sets per-rule values.

| Rule | Category | Fires | Clears |
|---|---|---|---|
| spo2.critical | RESPIRATORY | SpO₂ < 85 % for 15 | ≥ 90 % for 60 |
| spo2.warning | RESPIRATORY | SpO₂ < 90 % for 60 | ≥ 92 % for 60 |
| hr.critical.high | CARDIO | HR > 180 for 30 | ≤ 160 for 60 |
| hr.warning.high | CARDIO | HR > 130 for 120, at rest (activity ≤ 0.3; activity unknown → evaluated, SR-018) | ≤ 120 for 120 |
| hr.critical.low | CARDIO | HR < 40 for 30 | ≥ 45 for 60 |
| rr.critical.low | RESPIRATORY | resp. rate < 6 for 30 | ≥ 8 for 60 |
| rr.warning.high | RESPIRATORY | resp. rate > 30 for 120 | ≤ 25 for 120 |
| temp.warning | THERMAL | core temp > 38.0 for 600 | ≤ 37.7 for 600 |
| temp.critical | THERMAL | core temp > 39.5 for 300 | ≤ 39.0 for 300 |
| af.observe | CARDIO | AF probability (model M2) > 0.8 for 300 → observe notice "record a 12-lead ECG" | ≤ 0.5 for 600 |
| asystole.critical | CARDIO | Absence-of-event: no QRS for 6 s with good ECG SQI | QRS detected for 10 s |
| co2.warning | ENVIRONMENT | cabin CO₂ > 4 mmHg for 900 | ≤ 3.5 for 900 |
| o2.critical | ENVIRONMENT | cabin ppO₂ < 16 kPa for 30 | ≥ 17 for 60 |
| baseline.* | per parameter | robust z ≥ 4 for 600 (observe only) | z ≤ 2.5 for 600 |

Baseline rules apply to: heart_rate (at rest), hrv_rmssd, respiration_rate, core_temp, spo2 (at
rest), weight. `af.observe` stays at observe level until model M2 is `VALIDATED`; promoting it to
warning needs a CR. Open items for the medical lead (CR-002 §6): persistence for spot-check
readings (OI-A) and rules for `body_temp_measured` (OI-B).

Escalation (seconds after creation, stops on acknowledge): critical: astronaut 0, CMO 0, sound 15
(crew stations sound; wristband Immediate Alert in addition), crew 30, ground 60 (OPS channel,
path B). Warning: astronaut 0, CMO 0, crew 60. Observe: astronaut 0, reminder 3600, CMO 86400.

### 3a. Alarm content on the OPS channel (CR-002 B5)
Ground escalation always goes on OPS via path B. Allowed fields: crew id, priority, alarm
category, state, escalation stage, timestamps. "Physiology" means any measured value, parameter
name, rule id or free text; it never goes on OPS. Full detail follows on MEDICAL via path A or A'.

## 4. Hazard log (initial; maintained in `docs/hazards/` per ISO 14971)
| ID | Hazard | Cause examples | Controls |
|---|---|---|---|
| H-01 | Missed life-threatening event | Rule bug, data loss, suppressed alarm, app crash, wristband out of range | SR-001/002/007, crew stations always sound (SR-015), fail-over, watchdog, test corpus |
| H-02 | False reassurance from wrong/old value | Stale, fabricated or device-demonstration data; defaulted context | SR-004, SR-017, SR-018, R-CLN-02, staleness UI |
| H-03 | Alarm fatigue | Flicker, artefacts | SR-003/005, alarm statistics (F-ALM-09) |
| H-04 | Wrong crew member | Device mis-assignment | Registry assignment by CMO, pairing check, name on every alarm |
| H-05 | Privacy breach | Physiology on ops channel | R-SEC-06 allow-lists and physiology definition (§3a), encryption |
| H-06 | Malicious or corrupted update | Unsigned ruleset/model; dev-signed ruleset in a release build | R-SEC-05, SR-009 |
| H-07 | Unintended vehicle or device effect | Command path; unreviewed device write | SR-011, R-SEC-07 write-exception register, crate boundaries, CI guard |
| H-08 | Time error corrupts sequence | Device clock drift; wrong leap-second table | SR-012, R-OPS-05 |
| H-09 | Misleading model output | Unvalidated model | F-ML-01 gates, advisory labelling, SR-002, model rules observe-only until validated |
| H-10 | Use error under stress | Complex UI, gloves, low contrast | IEC 62366-1 process, glove mode, break-glass, contrast thresholds (`07` §1) |
| H-11 | Ground not told of an emergency | Medical data system down or batching | SR-015 dual routing; ground escalation on OPS via path B |
| H-12 | Double-counted or missed device data | Same device seen directly and via medical system | SR-016 de-duplication by source id, provenance |
