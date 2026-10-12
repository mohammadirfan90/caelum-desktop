# ICD-VEH (draft): CAELUM ↔ vehicle (cFS)

1. **Identification:** ICD-VEH v0.2; CAELUM team ↔ vehicle flight-software owner; F-GND-11,
   F-GND-13, F-ENV-01..03, F-ACQ-11; R-INT-04, R-OPS-05, R-SEC-04, R-SEC-06.
2. **Purpose and scope:** inbound vehicle data read by CAELUM (cabin atmosphere, radiation
   including solar-particle-event state, exercise devices, vehicle time); outbound CAELUM
   operational status and alarm telemetry (path B, OPS channel).
3. **Physical / transport:**
   - Inbound: UDP datagrams to the address and port in the signed MID table (`link.listen`),
     from the vehicle addresses in `link.allowed_sources` only. Two framings (`link.framing`):
     - `ccsds`: one CCSDS space packet per datagram (a cFS UDP telemetry output such as TO_LAB);
     - `sbn`: Software Bus Network UDP module datagrams. Header (11 bytes, big-endian): MsgSz
       (u16, bytes after the header), MsgType (u8), ProcessorID (u32), SpacecraftID (u32). Only
       MsgType 3 (application message, carrying one packet) is read; subscription, protocol and
       heartbeat messages are ignored and counted. `link.sbn_peers` optionally allow-lists
       ProcessorIDs. CAELUM never sends SBN subscriptions or heartbeats (it has no send path,
       SR-011), so the vehicle side must forward the configured MIDs statically (OI-5).
   - Outbound: **open item OI-1**: how CAELUM telemetry enters cFS (SBN peer, CI_LAB or a
     ground-link gateway). Until agreed, path B emits CCSDS packets to a configurable UDP
     endpoint (address and port from signed configuration).
4. **Protocol:** CCSDS 133.0-B-2 Space Packets.
   - Inbound packet: primary header (6 bytes, big-endian): version 0, packet type 0
     (telemetry; a telecommand is refused), secondary-header flag 1, APID (11 bits), sequence
     flags `11` (unsegmented; others refused), sequence count (14 bits), data length (total − 7;
     must equal the datagram). cFS telemetry secondary header: time, 4 bytes seconds + 2 bytes
     subseconds (2⁻¹⁶ s), big-endian, since the vehicle epoch. Header layout (`link.header`):
     `cfe7` = 4 spare bytes after the time, payload at byte 16 (cFE 7 "Draco" and later);
     `cfe6` = payload at byte 12.
   - MID = first 16 bits of the packet with the version bits cleared (cFE default mapping), so a
     telemetry MID is `0x0800 | APID`. Command MIDs (`0x1xxx`) cannot be configured.
   - Payload fields are in the producing processor's byte order, stated per message
     (`byte_order`).
   - Outbound: telemetry packets only (packet type bit 0) with a CUC time secondary header.
5. **Data definitions:**
   - Inbound MIDs and fields come from the signed MID table (configuration bundle kind
     `vehicle`, CR-015 §3), built from the vehicle mission database. Each field gives the
     catalog parameter, offset, type (`u8 i8 u16 i16 u32 i32 f32 f64`), unit (UCUM, a catalog
     unit; never assumed), optional linear calibration (`value = raw × scale + bias`) and an
     optional validity bit. Values are converted to the catalog's canonical unit and checked
     against its plausible range. Readable parameters per message kind:

     | Kind | Parameters | Subject | Mode |
     |---|---|---|---|
     | `environment` | `cabin_co2` (mmHg), `cabin_o2` (ppO₂, kPa), `cabin_pressure` (kPa), `cabin_temp` (°C), `cabin_humidity` (%) | cabin (optional `location` label) | continuous |
     | `radiation` | `radiation_dose_rate` (µSv/h), `radiation_dose_cumulative` (mSv), `spe_state` | cabin (area dosimeter) or crew member (personal dosimeter, `crew_field`) | continuous |
     | `exercise` | `exercise_load` (W), `exercise_duration` (min) | crew member (`crew_field` required) | spot (one per session) |
     | `time` | none: clock-set flag (`clock_state`) and optional leap seconds | vehicle | n/a |

     Dose values must be dose equivalent (Sv). Absorbed dose (Gy) has no catalog conversion and
     is refused at load time (OI-6). Vehicle crew IDs map to CAELUM crew members through the
     table's `[[crew]]` entries; an unmapped ID drops the packet.
   - `spe_state` values (OI-2, ⚕ DRAFT per CR-015 §4): `none`, `watch`, `warning`,
     `in_progress`, `all_clear`. The integer codes for them are in the MID table; an unmapped
     code is rejected.
   - **Reference layout** (v0.2, proposed to the vehicle owner; used by the test equipment and
     `crates/caelum-vehicle/tests/fixtures/reference.toml`). Little-endian payloads, cFE 7
     header. Validity byte: bit set = value valid.

     | MID | Name | Rate | Payload (byte offset: type field) |
     |---|---|---|---|
     | 0x0805 | CFE_TIME_HK_TLM | cFE HK rate | 2: u16 ClockStateFlags (bit 15 CLKSET); 6: i16 LeapSeconds (other cFE TIME HK fields not read) |
     | 0x0C40 | ECLSS_CABIN_ATMOS_TLM | 1 Hz | 0: f32 ppCO₂ mmHg; 4: f32 ppO₂ kPa; 8: f32 total pressure kPa; 12: f32 temperature °C; 16: f32 RH %; 20: u8 validity (bits 0–4 in field order); 21–23 spare |
     | 0x0C41 | RAD_AREA_TLM | 0.1 Hz | 0: f32 dose rate µSv/h; 4: f32 cumulative mSv; 8: u8 SPE code (0 none, 1 watch, 2 warning, 3 in_progress, 4 all_clear); 9: u8 validity (bits 0–2); 10–11 spare |
     | 0x0C42 | RAD_PERSONAL_TLM | 1/60 Hz per crew member | 0: u8 vehicle crew ID; 1: u8 validity (bit 0 rate, bit 1 cumulative); 2–3 spare; 4: f32 dose rate µSv/h; 8: f32 cumulative mSv |
     | 0x0C43 | EXERCISE_SESSION_TLM | end of each session | 0: u8 vehicle crew ID; 1–3 spare; 4: f32 mean load W; 8: u32 duration s |

   - Outbound: APIDs/MIDs from the signed APID plan. OPS fields only: crew id, priority, alarm
     category (CARDIO / RESPIRATORY / THERMAL / ENVIRONMENT / TECHNICAL), alarm state, escalation
     stage, timestamps, technical health. No physiology: no measured value, parameter name, rule id
     or free text. XTCE generated from the parameter catalog (F-GND-02).
6. **Timing:** CUC epoch 1958-01-01 TAI (CCSDS epoch). MET epoch from signed mission
   configuration. UTC derived from TAI with the bundled IERS leap-second table, updated only by
   signed configuration. Status packet rate (OI-3); alarm packets on every state change.
   - **Inbound time correlation (F-ACQ-11):** the vehicle epoch and scale are in the MID table
     (`time.epoch` as `YYYY-MM-DDTHH:MM:SS`, `time.scale` `tai` or `utc`; reference: 1980-01-06,
     TAI). A TAI-scale time is epoch + seconds; a UTC-scale time goes through the signed
     leap-second table (and is flagged inexact past the table's expiry).
   - Vehicle time is a reading's measurement time only while the configured time message
     reports the clock set and that report is not older than `time.clock_state_max_age_s`.
     Otherwise (no time message configured, clock not set, no or stale report) the reading gets
     the station receipt time, marked `receipt_unsynchronised` (docs/05 §4).
   - A vehicle time more than 1 s after station receipt is a clock fault: the packet is dropped
     and counted (`time_in_future`).
   - Correlation status: clock state, receipt − vehicle offset (latest, minimum and maximum over
     the last 64 packets), latest vehicle time and its MET, vehicle and station TAI−UTC.
7. **Direction and authority:** inbound is read-only. Outbound is telemetry only: CAELUM builds
   no telecommand packets and uses no command MIDs. `caelum-vehicle` has no write API; the
   publisher lives in `caelum-ground` (SR-011, R-SEC-04). The receiver's public API is receive
   only (`tests/no_send_api.rs`), and the CI write guard scans the crate with no exceptions.
8. **Security:** link authentication (OI-4); allow-listed source addresses, SBN processor IDs
   and MIDs inbound; allow-listed MIDs outbound; OPS carries no physiology (R-SEC-06). The MID
   table is accepted only as a verified signed bundle.
9. **Error handling:** malformed inbound packets are rejected and counted by reason
   (`source_not_allowed`, `too_short`, `ccsds_version`, `telecommand`, `no_secondary_header`,
   `segmented`, `length_mismatch`, `sbn_control`, `sbn_peer`, `unknown_mid`,
   `payload_too_short`, `time_unconvertible`, `time_in_future`, `unmapped_crew`); field-level
   faults drop only that field (`sensor_invalid`, `not_finite`, `implausible`,
   `unknown_state`). Sequence-count gaps and the last receipt time are kept per MID. Inbound
   gaps make values STALE and raise a technical alarm (SR-004); outbound packets wait in the
   store-and-forward queue while the endpoint is unavailable.
10. **Safety:** H-07, H-11; SR-011, SR-015. Environment rules (`co2.warning`, `o2.critical`)
    evaluate vehicle readings like any other observation.
11. **Verification:** cFS packet source in test equipment (`caelum-test-equipment::cfs`,
    encoder written independently of the decoder); decoder tests for every message kind and
    every rejection reason; UDP loopback end-to-end test with CO₂ above `co2.warning` (M9);
    write-exception CI guard; outbound packets decoded by Yamcs with the generated XTCE (M10);
    a test that the publisher cannot set the telecommand bit (M10).
12. **Open items:** OI-1 entry path into cFS; OI-2 `spe_state` values (DRAFT in CR-015 §4,
    awaiting the vehicle owner and medical lead); OI-3 status packet rate; OI-4 link
    authentication; OI-5 static SBN forwarding of the configured MIDs (no subscription traffic
    from CAELUM), SBN header to be confirmed against the vehicle's SBN version; OI-6 radiation
    quantities: vehicle to publish dose equivalent (Sv), not absorbed dose; OI-7 the vehicle
    mission database (real MIDs, layouts, rates, crew IDs) to replace the reference layout.
    **Change history:** v0.1 created by CR-002; v0.2 inbound sections completed by CR-015.
    **Approvals:** pending.
