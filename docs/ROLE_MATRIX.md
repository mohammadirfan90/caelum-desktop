# Role matrix

Defines who may see and do what (F-SYS-01, R-SEC-02, CLAUDE.md rule 6). Every IPC command and
network endpoint is checked against this matrix; M1 adds a test per command and role. Principles
from CR-003 §B2.

**Status:** approved 2026-10-10. The product owner delegated the choices to engineering (§5
records each decision and why). ⚕ Clinical-data access (§2) is DRAFT under the provisional medical
lead (CR-003) until a licensed clinician advisor countersigns; it does not block the build.

## 1. Roles and scope
| Role | Where | Scope of crew data |
|---|---|---|
| Astronaut | onboard | Own data only |
| Crew Medical Officer (CMO) | onboard | All crew onboard |
| Flight Surgeon | ground | All crew |
| Biomedical Engineer (BME) | ground | Devices, technical alarms, data quality; clinical values read-only |
| Mission Control Operator (MCC) | ground | Operational status only (OPS fields, `06` §3a) |
| Administrator | onboard / ground | Configuration only; no clinical content (separation of duties) |
| Device Gateway | machine identity | Ingest only; no reads |
| Break-glass (no login) | any crew station | Crew-wide emergency view and protocols only (SR-014); every use audited and notified |

## 2. Data access (R = read, W = write, — = none)
| Data | Astronaut | CMO | Flight Surgeon | BME | MCC | Admin | Gateway |
|---|---|---|---|---|---|---|---|
| Vital signs, observations, waveforms | R own | R all | R all | R all | — | — | W (ingest) |
| Physiological alarm detail (values, why) | R own | R all | R all | R all | — | — | — |
| Alarm status (OPS fields only) | R own | R all | R all | R all | R all | — | — |
| Alarm actions: acknowledge, silence | own | all | ground stage only (§5.4) | — | — | — | — |
| Technical alarms (sensor, device, storage) | R own, ack own | R/ack all | R/ack all | R/ack all | R status | R | — |
| Encounters, SOAP notes, attachments | R own (signed) | R/W all | R/W all | — | — | — | — |
| Daily check-in: Private tier | R/W own | — | — | — | — | — | — |
| Daily check-in: Medical tier | R/W own | R all | R all | — | — | — | — |
| Daily check-in: Status tier | R/W own | R all | R all | — | R (level only) | — | — |
| Device registry, assignment, calibration | R own devices | R/W | R | R/W | — | R/W | — |
| Configuration bundles (signed) | — | — | — | — | — | R/W (apply needs re-auth) | — |
| Users and roles | R self | R | R | R | R self | R/W (re-auth) | — |
| Audit log | — | R (all entries) | R (all entries) | R (technical entries) | — | R (configuration and security entries only) | — |
| Ground: downlink queue, uplink preparation | — | — | R/W | R/W | R (OPS queue) | R | — |
| Model outputs (advisory) | R own | R all | R all | R all | — | — | — |

## 3. Commands
| Command | Roles allowed | Notes |
|---|---|---|
| `app_info` | public (no login) | Product name and version only |
| `auth_login` | public | Rate-limited; failures audited |
| `auth_logout` | any signed-in user | |
| `auth_me` | any signed-in user | Own identity, role, scope |
| `auth_reauth` | any signed-in user | Required before sensitive actions (§4) |
| `break_glass_open` | no login | Opens the emergency view; audited and notified (SR-014) |
| `audit_list` | CMO, Flight Surgeon (all); BME (technical); Admin (configuration and security) | Filtered by role server-side |
| `audit_verify` | CMO, Flight Surgeon, BME, Admin | Returns chain and signature status only |
| `alarm_acknowledge` | Astronaut (own), CMO (all), Flight Surgeon (ground stage only) | §2 alarm actions; the flight surgeon's acknowledgement is recorded for the ground stage and never stops onboard escalation (§5.4); audited (F-ALM-05) |
| `alarm_silence` | Astronaut (own), CMO (all) | Time-limited (SR-008: 2 min critical, 10 min warning); never removes the visual alarm; audited |
| `alarm_escalate` | Astronaut (own), CMO (all) | Manual escalation to the next stage (F-ALM-05); audited |
| `setup_status` | public | Whether this installation has any account yet (CR-007 §4) |
| `setup_create_administrator` | public, only while no account exists | Creates the first administrator once; refused as soon as any account exists; audited (CR-007 §4) |
| `ruleset_status` | public | Ruleset version in force and the DEVELOPMENT RULESET / fallback / safe-state banner (SR-009); safety information shown before sign-in, no clinical content |
| `crew_list` | Astronaut (self); CMO, Flight Surgeon, BME, Admin (all) | Roster names and positions only, no health data (CR-012 §2) |
| `config_apply_crew` | Admin | Signed crew-roster bundle (F-SYS-07); re-authentication (§4); audited |
| `user_list` | Astronaut (self); MCC (self); CMO, Flight Surgeon, BME, Admin (all) | §2 "Users and roles"; never credentials |
| `user_create` | Admin | Re-authentication (§4); CR-007 policy; audited |
| `user_disable` | Admin | Re-authentication (§4); never the caller's own account, so never the last administrator; audited |
| `vitals_latest` | Astronaut (own); CMO, Flight Surgeon, BME (all) | §2 vital signs; value with source, time, quality and status (CR-012 §4) |
| `vitals_trend` | Astronaut (own); CMO, Flight Surgeon, BME (all) | At most 24 h, thinned to 2000 points; baseline if one exists |
| `waveform_recent` | Astronaut (own); CMO, Flight Surgeon, BME (all) | The last 30 s at most |
| `alarm_list` | Astronaut (own); CMO, Flight Surgeon, BME (all); MCC (OPS fields only for every crew member) | Open alarms and those ended in the last 24 h (docs/06 §3a for MCC) |
| `alarm_detail` | Astronaut (own); CMO, Flight Surgeon, BME (all) | §2 physiological alarm detail |
| `alarm_report` | CMO, Flight Surgeon, BME | F-ALM-09, CR-012 §5 |
| `device_list` | Astronaut (own devices); CMO, Flight Surgeon, BME, Admin (all) | §2 device registry with battery, link state and clock offset |
| `device_assign` | CMO, BME, Admin | Re-authentication (§4); roster member in force; audited |
| `device_unassign` | CMO, BME, Admin | Re-authentication (§4); audited |
| `device_scan` | CMO, BME, Admin | Bluetooth scan for health services (F-ACQ-01) |
| `device_pair` | CMO, BME, Admin | Pairs and registers a device; audited |

## 4. Sessions
- Onboard: PIN plus device-bound key; auto-lock after 10 minutes (configurable). Ground: password
  plus key.
- Re-authentication within the last 2 minutes is required for: signing an encounter, applying a
  configuration bundle or ruleset, changing users or roles, assigning a device to a crew member.
- A locked session keeps alarms visible and audible (alarms never depend on login). While locked,
  only `auth_me`, `auth_reauth` (which unlocks) and `auth_logout` are available; every
  role-restricted command needs an unlocked session (CR-007 §3).

## 5. Decisions (delegated by the product owner, 2026-10-10)
1. **Astronauts acknowledge and silence their own alarms; the CMO for anyone onboard.** BME, MCC
   and administrator cannot act on physiological alarms. Why: the person affected and the onboard
   medical officer are the ones who can respond; others acting would hide an alarm from them.
2. **BME reads clinical values but cannot act on physiological alarms.** Why: device and data
   quality work needs the values; clinical decisions stay with clinicians.
3. **Administrator sees only configuration and security audit entries, never clinical ones.** Why:
   separation of duties (CR-003); configuration work never needs health data.
4. **The ground flight surgeon acknowledges only the ground escalation stage** ("seen by ground");
   it never acknowledges or silences the alarm onboard, and onboard escalation never waits for
   ground. Why: with communication delay or loss of signal, responsibility for an onboard
   emergency must stay with the crew; a remote acknowledgement must not stop crew notification
   (R-ALM-03, SR-007, SR-015).

## 6. Navigation (UI, derived from §2; M5)
Screens in the nav rail per role. A screen is shown only where §2 grants the data it needs.

| Screen | Astronaut | CMO | Flight Surgeon | BME | MCC | Admin |
|---|---|---|---|---|---|---|
| Home | yes | yes | yes | yes | — | — |
| Body | yes | yes | yes | yes | — | — |
| Alerts | yes | yes | yes | yes | yes | — |
| Checks | yes | yes | — | — | — | — |
| Env | yes | yes | yes | yes | yes | — |
| Medical | — | yes | yes | — | — | — |
| Devices | yes | yes | yes | yes | — | yes |
| Ground | — | — | yes | yes | yes | yes |
| Settings | yes | yes | yes | yes | yes | yes |

