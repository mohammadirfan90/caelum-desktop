# ICD-VEH (draft): CAELUM ↔ vehicle (cFS)

1. **Identification:** ICD-VEH v0.1; CAELUM team ↔ vehicle flight-software owner; F-GND-11,
   F-GND-13, F-ENV-01..03, F-ACQ-11; R-INT-04, R-OPS-05, R-SEC-04, R-SEC-06.
2. **Purpose and scope:** inbound vehicle data read by CAELUM (cabin atmosphere, radiation
   including solar-particle-event state, exercise devices, vehicle time); outbound CAELUM
   operational status and alarm telemetry (path B, OPS channel).
3. **Physical / transport:**
   - Inbound: cFS Software Bus through the Software Bus Network (SBN) or a configured UDP
     telemetry output. Message IDs come from signed configuration.
   - Outbound: **open item OI-1**: how CAELUM telemetry enters cFS (SBN peer, CI_LAB, or a
     ground-link gateway). Until agreed, path B emits CCSDS packets to a configurable UDP
     endpoint (address and port from signed configuration).
4. **Protocol:** CCSDS 133.0-B-2 Space Packets. Inbound: cFS message format (CCSDS primary header
   and cFS secondary header). Outbound: telemetry packets only (packet type bit 0) with a CUC time
   secondary header.
5. **Data definitions:**
   - Inbound: MIDs and fields per the vehicle mission database (to be supplied by the vehicle
     owner); `spe_state` enumeration values (OI-2).
   - Outbound: APIDs/MIDs from the signed APID plan. OPS fields only: crew id, priority, alarm
     category (CARDIO / RESPIRATORY / THERMAL / ENVIRONMENT / TECHNICAL), alarm state, escalation
     stage, timestamps, technical health. No physiology: no measured value, parameter name, rule id
     or free text. XTCE generated from the parameter catalog (F-GND-02).
6. **Timing:** CUC epoch 1958-01-01 TAI (CCSDS epoch). MET epoch from signed mission
   configuration. UTC derived from TAI with the bundled IERS leap-second table, updated only by
   signed configuration. Status packet rate (OI-3); alarm packets on every state change.
7. **Direction and authority:** inbound is read-only. Outbound is telemetry only: CAELUM builds
   no telecommand packets and uses no command MIDs. `caelum-vehicle` has no write API; the
   publisher lives in `caelum-ground` (SR-011, R-SEC-04).
8. **Security:** link authentication (OI-4); allow-listed MIDs in both directions; OPS carries no
   physiology (R-SEC-06).
9. **Error handling:** malformed inbound packets are rejected and counted; inbound gaps make
   values STALE and raise a technical alarm (SR-004); outbound packets wait in the
   store-and-forward queue while the endpoint is unavailable.
10. **Safety:** H-07, H-11; SR-011, SR-015.
11. **Verification:** cFS packet source in test equipment (M9); write-exception CI guard;
    outbound packets decoded by Yamcs with the generated XTCE (M10); a test that the publisher
    cannot set the telecommand bit (M10).
12. **Open items:** OI-1 entry path into cFS; OI-2 `spe_state` values; OI-3 status packet rate;
    OI-4 link authentication. **Change history:** v0.1 created by CR-002. **Approvals:** pending.
