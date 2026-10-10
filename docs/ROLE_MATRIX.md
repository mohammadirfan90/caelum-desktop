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

## 3. M1 commands
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
| `ruleset_status` | public | Ruleset version in force and the DEVELOPMENT RULESET / fallback / safe-state banner (SR-009); safety information shown before sign-in, no clinical content |

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
