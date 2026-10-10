# ICD-SENSOR-KIT: CAELUM sensor shirt and wristband ↔ CAELUM station (BLE)

**Status: DRAFT v1, approved by engineering under the product owner's standing delegation
(CR-013); firmware must conform.** Implemented exactly by `crates/caelum-devices/src/sensor_kit.rs`.

1. **Identification:** ICD-SENSOR-KIT v1; CAELUM station team ↔ sensor-kit firmware; F-ACQ-05,
   F-ACQ-10, F-SIG-05 (heat-flux input); SR-017, R-SEC-07, SR-011.
2. **Purpose and scope:** physiological waveforms and slow measurements from the CAELUM sensor
   shirt (ECG 1–3 leads, RIP thoracic and abdominal, IMU, skin temperature proximal and distal,
   heat flux) and wristband (PPG red/IR/green/ambient, IMU, EDA, skin temperature distal), plus
   device identity, calibration constants and status. Inbound to CAELUM only.
3. **Physical / transport:** Bluetooth LE, bonded and encrypted link (pairing per F-ACQ-01).
   ATT_MTU 247 requested; frames are at most 244 bytes (ATT_MTU − 3). One vendor GATT service:

   | Item | UUID | Properties |
   |---|---|---|
   | Sensor-kit service | `cd8c0001-b900-4563-a6dc-fd325d5f51cd` | primary |
   | Data | `cd8c0002-b900-4563-a6dc-fd325d5f51cd` | notify; one frame per notification |
   | Info | `cd8c0003-b900-4563-a6dc-fd325d5f51cd` | read |

   The UUIDs derive from one random v4 base (`cd8c4c29-b900-4563-a6dc-fd325d5f51cd`) by replacing
   the low 16 bits of the first field. **No writable characteristic exists in v1** (only the CCCD
   write that enables notifications, a GATT client operation). Any future write needs a reviewed
   entry in `docs/write-exceptions.md` first (CLAUDE.md rule 5).
4. **Protocol:** all integers little-endian. Frames are self-contained; there is no
   segmentation. A frame with a bad CRC, an unknown version or type, a reserved bit set, or a
   length that does not match its type is rejected whole: no partial output.
5. **Data definitions.**

   **5.1 Frame header (13 bytes) and trailer (2 bytes)**

   | Offset | Size | Field |
   |---|---|---|
   | 0 | u8 | Protocol version = 1 (other values rejected) |
   | 1 | u8 | Frame type (5.2–5.8) |
   | 2 | u16 | Sequence number, per device, +1 per frame, wraps 0xFFFF → 0 |
   | 4 | u8 | Status flags: bit 0 demonstration data; bit 1 test data; bit 2 sensor off / lead off; bit 3 saturation / clipping; bit 4 device clock not set; bits 5–7 reserved, 0 |
   | 5 | u64 | Device time of the first sample: TAI nanoseconds since the CCSDS epoch 1958-01-01T00:00:00 TAI on the device clock (same epoch as `TaiNs`); 0 = not set; must be ≤ 2^63 − 1 |
   | 13 | n | Type-specific payload |
   | 13 + n | u16 | CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF, no reflection, xorout 0) over bytes 0 … 12 + n |

   The status flags are **mandatory** in every frame: firmware must set bit 2 whenever a sensor
   used by the frame has no contact and bit 3 whenever any sample clipped. CAELUM therefore
   assigns quality `good` to a frame that flags nothing, `poor` on bit 3 and `unusable` on bit 2
   (bit 2 wins). Bits 0–1 set the SR-017 device flags (rejected in release builds). Bit 4 or a
   zero timestamp: no device time (the station's receipt time applies). In status frames (0x07)
   bits 2–3 must be 0. Clock offset for the device time is measured per F-ACQ-11.

   Payload limit: 244 − 15 = 229 bytes. Sample rates are integer Hz, > 0. Every waveform frame
   carries at least one whole sample set; the payload after the type header must be an exact
   multiple of the sample-set size.

   **5.2 0x01 ECG** — samples in mV = raw × resolution / 10^6.

   | Offset | Size | Field |
   |---|---|---|
   | 13 | u16 | Sample rate, Hz |
   | 15 | u8 | Lead count N, 1–3 |
   | 16 | N × u8 | Lead ids, distinct: 1 I, 2 II, 3 III, 4–9 V1–V6 |
   | 16 + N | u16 | Resolution, nV per LSB (> 0) |
   | 18 + N | k × N × i16 | Samples, interleaved by lead in lead-id order of the header |

   **5.3 0x02 RIP** — raw counts, relative and uncalibrated (no physical unit).

   | Offset | Size | Field |
   |---|---|---|
   | 13 | u16 | Sample rate, Hz |
   | 15 | u8 | Channel mask: bit 0 thoracic, bit 1 abdominal; others 0; not 0 |
   | 16 | k × C × u16 | Samples, interleaved in bit order (C = channels set) |

   **5.4 0x03 IMU** — acceleration in g = raw × µg/LSB / 10^6; rate in °/s = raw × m°/s per LSB / 1000.

   | Offset | Size | Field |
   |---|---|---|
   | 13 | u16 | Sample rate, Hz |
   | 15 | u16 | Accelerometer resolution, µg per LSB (> 0) |
   | 17 | u16 | Gyroscope resolution, m°/s per LSB (> 0) |
   | 19 | k × 6 × i16 | Samples: ax ay az gx gy gz |

   **5.5 0x04 PPG** — raw ADC counts, unsigned 24-bit (AFE resolutions up to 24 bits).

   | Offset | Size | Field |
   |---|---|---|
   | 13 | u16 | Sample rate, Hz |
   | 15 | u8 | Channel mask: bit 0 red, bit 1 infrared, bit 2 green, bit 3 ambient; others 0; not 0 |
   | 16 | k × C × u24 | Samples, interleaved in bit order |

   **5.6 0x05 Temperature and heat flux** (one measurement per frame; payload exactly 6 bytes).
   0x8000 = not measured: no value is produced for that field.

   | Offset | Size | Field | Output |
   |---|---|---|---|
   | 13 | i16 | Skin temperature proximal, 0.01 °C | `skin_temp_proximal` |
   | 15 | i16 | Skin temperature distal, 0.01 °C | `skin_temp_distal` |
   | 17 | i16 | Heat flux, 0.1 W/m², positive out of the body | F-SIG-05 input only; no catalog parameter |

   **5.7 0x06 EDA** (payload exactly 2 bytes): u16 skin conductance in 0.01 µS at offset 13
   → `eda`; 0xFFFF = not measured.

   **5.8 0x07 Device status** (payload exactly 3 bytes):

   | Offset | Size | Field |
   |---|---|---|
   | 13 | u8 | Battery, % (0–100); 0xFF unknown → `battery_level` |
   | 14 | u8 | Contact supported: bit 0 ECG electrodes, 1 RIP thoracic, 2 RIP abdominal, 3 PPG, 4 EDA electrodes, 5 skin temp proximal, 6 skin temp distal, 7 heat flux |
   | 15 | u8 | Contact detected (same bits; a bit set here must be set in "supported") |

   **5.9 Info characteristic** (read; at most 512 bytes):

   | Offset | Size | Field |
   |---|---|---|
   | 0 | u8 | Protocol version = 1 |
   | 1 | u8 | Device kind: 1 sensor shirt, 2 wristband |
   | 2 | u8, u8, u16 | Firmware version major, minor, patch |
   | 6 | 16 bytes | Serial number, printable ASCII, NUL-padded, not empty |
   | 22 | u8 | Calibration record count M |
   | 23 | M × 6 | Records: channel u8, kind u8 (1 gain, 2 offset), value IEEE 11073-20601 FLOAT (special values rejected) |
   | 23 + 6M | u16 | CRC-16/CCITT-FALSE over all preceding bytes |

   Channel ids: 1 ECG, 2 RIP thoracic, 3 RIP abdominal, 4 accelerometer, 5 gyroscope, 6 PPG,
   7 EDA, 8 skin temp proximal, 9 skin temp distal, 10 heat flux. Firmware applies calibration
   before sending; CAELUM records the constants for provenance and maintenance and never applies
   them again.
6. **Timing:** device time per 5.1; all samples in a frame are evenly spaced at the frame's
   sample rate from the first-sample time. Readings from 0x05–0x07 are continuous-mode.

   **Example frames** (both verified by unit tests). Device time 2026-01-01T00:00:00 UTC =
   2 145 916 837 000 000 000 ns TAI since 1958 (`00 32 EA 62 FE D3 C7 1D`).
   - EDA 5.00 µS, sequence 1, no flags:
     `01 06 01 00 00 00 32 EA 62 FE D3 C7 1D F4 01 | 52 4A` (CRC 0x4A52).
   - Temperature, sequence 42: proximal 34.12 °C, distal not measured, heat flux −12.5 W/m²:
     `01 05 2A 00 00 00 32 EA 62 FE D3 C7 1D 54 0D 00 80 83 FF | BD BD` (CRC 0xBDBD).
7. **Direction and authority:** read-only. CAELUM subscribes to Data and reads Info; it writes
   nothing to the device (R-SEC-07, SR-011).
8. **Security:** LE Secure Connections bonding and link encryption (F-ACQ-01); the CRC detects
   corruption and format errors, not tampering.
9. **Error handling:** rejected frames are counted and logged. Sequence gaps count as dropped
   frames (F-ACQ-10); a repeated number is a duplicate and is dropped; a number that goes
   backwards (or jumps by exactly 0x8000) is a counter restart and resets tracking without
   counting drops. Tracking restarts on reconnection.
10. **Safety:** SR-017 (demonstration/test data rejected in release builds); lead-off and
    saturation make data unusable/poor so physiological rules do not evaluate it (SR-005).
11. **Verification:** unit tests in `sensor_kit.rs` (CRC check value, every frame type, flag
    mapping, sentinels, truncation at every length, sequence gaps/duplicates/wrap); firmware
    conformance against the example frames.
12. **Open items:** OI-1 firmware conformance test vectors from the firmware team; OI-2 whether
    fractional sample rates are needed (would need protocol version 2). **Change history:** v1
    created under CR-013. **Approvals:** engineering (standing delegation, CR-013).
