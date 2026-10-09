# 06. Safety and alarms

## 1. Principles (safety requirements)
| ID | Safety requirement |
|---|---|
| SR-001 | Absolute-limit rules are evaluated for every valid observation; nothing can disable them except a signed ruleset change. |
| SR-002 | Baselines and models can add lower-priority notices but can never suppress, delay or downgrade an absolute-limit alarm. |
| SR-003 | A rule fires only after its persistence window; it resolves only after its clear threshold holds for its clear window (hysteresis). |
| SR-004 | A data gap longer than `max_gap_s` restarts persistence timing; stale data raises a technical alarm, never a "normal" state. |
| SR-005 | Observations with quality POOR/UNUSABLE pause physiological rules for that signal and raise a technical alarm. |
| SR-006 | Alarm state is per crew member and per rule; a higher-priority alarm supersedes lower ones for the same parameter. |
| SR-007 | High-priority alarms escalate on timers until acknowledged; escalation state survives restart and fail-over. |
| SR-008 | Silencing is time-limited (max 2 min high, 10 min medium), never removes the visual alarm, and is audited. |
| SR-009 | The ruleset is loaded only if its signature and approver record verify; otherwise the app starts in a safe state that shows a technical alarm and uses the last verified ruleset. |
| SR-010 | Every alarm records why it fired: rule id, version, values, durations, device and quality. |
| SR-011 | No code path sends commands to vehicle systems or therapeutic devices. |
| SR-012 | Clock faults (offset > 50 ms or time going backwards) raise a technical alarm and are corrected in provenance. |
| SR-013 | Alarm sounds and visuals meet NASA-STD-3001 Vol. 2 / IEC 60601-1-8 distinguishability; tested in usability evaluation. |
| SR-014 | Break-glass access to the emergency view and protocols never requires a login. |
| SR-015 | Alarm notification to crew stations, wristbands and the operational ground path (cFS → downlink) is independent of the onboard medical data system. |
| SR-016 | Observations read from the medical data system go through the same ingest validation and alarm evaluation as direct device data, with duplicates removed by source id. |

## 2. Priorities and presentation
| CAELUM level | IEC 60601-1-8 priority | Shape | Sound | Latching |
|---|---|---|---|---|
| Critical | High | Octagon | High-priority pattern, repeating | Latched until acknowledged |
| Warning | Medium | Triangle | Medium-priority pattern | Latched until acknowledged |
| Observe | Low | Diamond | Single low tone (optional) | Non-latching |
| Advisory (technical) | Low / technical | Square with "!" | Technical tone | Clears when cause clears |
| Normal | — | Circle | — | — |

## 3. Initial ruleset (DRAFT, needs flight-surgeon approval before use)
Ported from the prototype's tested engine; persistence in seconds.
| Rule | Fires | Clears |
|---|---|---|
| spo2.critical | SpO₂ < 85 % for 15 | ≥ 90 % for 60 |
| spo2.warning | SpO₂ < 90 % for 60 | ≥ 92 % for 60 |
| hr.critical.high | HR > 180 for 30 | ≤ 160 for 60 |
| hr.warning.high | HR > 130 for 120, at rest (activity ≤ 0.3) | ≤ 120 for 120 |
| hr.critical.low | HR < 40 for 30 | ≥ 45 for 60 |
| rr.critical.low | resp. rate < 6 for 30 | ≥ 8 for 60 |
| rr.warning.high | resp. rate > 30 for 120 | ≤ 25 for 120 |
| temp.warning / critical | core temp > 38.0 for 600 / > 39.5 for 300 | ≤ 37.7 / ≤ 39.0 |
| af.warning | AF probability > 0.8 for 300 | ≤ 0.5 for 600 |
| asystole.critical | No QRS for 6 s with good ECG SQI | QRS detected for 10 s |
| co2.warning | cabin CO₂ > 4 mmHg for 900 | ≤ 3.5 for 900 |
| o2.critical | cabin ppO₂ < 16 kPa for 30 | ≥ 17 for 60 |
| baseline.* | robust z ≥ 4 for 600 (observe only) | z ≤ 2.5 for 600 |
Escalation (seconds after creation, stops on acknowledge): critical: astronaut 0, CMO 0, sound 15,
crew 30, ground 60. Warning: astronaut 0, CMO 0, crew 60. Observe: astronaut 0, reminder 3600,
CMO 86400.

## 4. Hazard log (initial; maintained in `docs/hazards/` per ISO 14971)
| ID | Hazard | Cause examples | Controls |
|---|---|---|---|
| H-01 | Missed life-threatening event | Rule bug, data loss, suppressed alarm, app crash | SR-001/002/007, fail-over, watchdog, test corpus |
| H-02 | False reassurance from wrong/old value | Stale or fabricated data | SR-004, R-CLN-02, staleness UI |
| H-03 | Alarm fatigue | Flicker, artefacts | SR-003/005, alarm statistics (F-ALM-09) |
| H-04 | Wrong crew member | Device mis-assignment | Registry assignment by CMO, pairing check, name on every alarm |
| H-05 | Privacy breach | Raw data on ops channel | R-SEC-06 allow-lists, encryption |
| H-06 | Malicious or corrupted update | Unsigned ruleset/model | R-SEC-05, SR-009 |
| H-07 | Unintended vehicle effect | Command path | SR-011, crate boundaries, CI check |
| H-08 | Time error corrupts sequence | Device clock drift | SR-012 |
| H-09 | Misleading model output | Unvalidated model | F-ML-01 gates, advisory labelling, SR-002 |
| H-10 | Use error under stress | Complex UI, gloves | IEC 62366-1 process, glove mode, break-glass |
| H-11 | Ground not told of an emergency | Medical data system down or batching | SR-015 dual routing, operational path direct via cFS |
| H-12 | Double-counted or missed device data | Same device seen directly and via medical system | SR-016 de-duplication by source id, provenance |
