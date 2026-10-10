# Write-exception register (R-SEC-07, SR-011, CR-002 D1)

`caelum-vehicle` has no write API. CAELUM status telemetry is built by `caelum-ground` with a
telemetry-only packet type. Device adapters in `caelum-devices` may write to a device only through
a call listed in this register. The CI guard (`tools/ci/check-writes.ts`, rules in
`tools/ci/src/writes.ts`) fails on any write call in `caelum-vehicle` or `caelum-devices` that is
not listed here, on any write at all in `caelum-vehicle`, and on any listed entry whose code
marker is missing. The guard scans source text for the write APIs of the transports these crates
use (std/tokio I/O, btleplug, bluer, hidapi, sockets); it is a tripwire, and the reviewer of each
entry remains responsible for the call it covers.

## Allowed categories
- **(a)** Bluetooth SIG control writes needed for acquisition: CCCD notification or indication
  enable, Record Access Control Point (RACP), standard measurement-interval writes.
- **(b)** Serial poll requests defined in a device ICD.
- **(c)** Wristband alert through the Bluetooth Immediate Alert Service (Alert Level 0x2A06) only.
  Crew stations always sound as well; no alarm depends on the wristband.

Anything else, and in particular any write to a therapeutic device or a vehicle system, is not
allowed. Changing these categories needs a change request.

## Entry rules
- One entry per call site, with ID `WX-NNN`.
- The call site carries the comment `// WRITE-EXCEPTION: WX-NNN`.
- An entry is valid only after a reviewer other than its author is recorded.
- Removing a call removes its entry in the same commit.

## Register
| ID | Crate | Call (module::function) | Category | Standard / ICD reference | Reason | Reviewer | Date |
|---|---|---|---|---|---|---|---|
| WX-001 | caelum-devices | `ble::enable_notifications` (btleplug `subscribe`) | a | CCCD 0x2902, Bluetooth Core Specification Vol 3 Part G §3.3.3.3; HRS, PLXS, BPS, HTS, WSS, GLS service specifications | Enables notifications (0x0001) or indications (0x0002) on the supported measurement characteristics only (`ble::measurement_characteristics`: 0x2A37, 0x2A5E, 0x2A5F, 0x2A35, 0x2A1C, 0x2A9D, 0x2A18); needed to receive data | Product owner (mohammadirfan90) | 2026-10-10 |

M0 shipped with zero exceptions; WX-001 added in M2.
