# 11. Interface Control Document template

Copy to `docs/icd/ICD-<interface>.md` before building any external interface.

1. **Identification:** interface name, ID, version, owners on both sides, related feature IDs.
2. **Purpose and scope:** what crosses the interface and why.
3. **Physical / transport:** link (BLE, USB, UDP, TCP, file, SBN), addressing, ports, MTU, rates.
4. **Protocol:** standard and edition (e.g. CCSDS 133.0-B-2), options used, deviations.
5. **Data definitions:** every message/packet/file: fields, types, units (UCUM), ranges, encoding,
   byte order, APIDs / MIDs / UUIDs, XTCE or schema reference.
6. **Timing:** rates, latency limits, time reference (TAI/UTC/MET), clock correlation.
7. **Direction and authority:** read-only or not; explicit statement that no commands are sent
   where applicable.
8. **Security:** authentication, encryption, signing, key management, channel allow-lists.
9. **Error handling:** malformed data, loss, duplicates, reordering, recovery.
10. **Safety:** hazards affected (H-xx) and controls.
11. **Verification:** tests, test equipment, interoperability evidence.
12. **Change history and approvals.**
