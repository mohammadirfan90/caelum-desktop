# ICD-MDS (draft): CAELUM ↔ onboard medical data system

1. **Identification:** ICD-MDS v0.1; CAELUM team ↔ medical data system owner; F-GND-12/13/14.
2. **Purpose:** keep the onboard health record complete and current; let CAELUM read devices the
   medical system collects directly.
3. **Transport:** crew LAN, HTTPS (TLS 1.3, mutual certificates), FHIR R4 REST base URL from signed config.
4. **Protocol:** HL7 FHIR R4; JSON; conditional create with `identifier` = CAELUM client id.
5. **Data:** Observation (LOINC + UCUM, IEEE 11073 device metric codes), DiagnosticReport,
   Encounter, Media / DocumentReference, Device, Provenance. Profiles to be agreed.
6. **Timing:** summaries every 60 s (configurable); alarm-related resources within 5 s of creation;
   read-back polling 10 s or subscription if supported.
7. **Direction:** two-way data; no commands in either direction.
8. **Security:** machine identity per instance; least-privilege scopes; audit on both sides.
9. **Errors:** idempotent retries with back-off; 4xx logged and held for review; 5xx/timeouts retried;
   outage > configured limit raises a CAELUM technical alarm.
10. **Safety:** H-11, H-12; SR-015, SR-016.
11. **Verification:** HAPI FHIR container as stand-in; real system in integration tests when available.
12. **Approvals:** pending.
