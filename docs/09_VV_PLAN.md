# 09. Verification and validation

## 1. Test levels
| Level | What | Tools |
|---|---|---|
| Unit | Every decoder, rule, filter, parser, repository | `cargo nextest`, `proptest` (property tests for parsers and rule timing) |
| Rule engine | Every rule: fire, not-fire, persistence edge, gap restart, hysteresis, supersession, multi-crew | table-driven tests + recorded corpora; 100 % branch coverage of `caelum-alarms` |
| Golden files | XTCE output, CCSDS packets, FHIR bundles, audit entries | `insta` snapshots |
| Integration | Device emulator → ingest → alarm → escalation → downlink → ground profile decode | `caelum-test-equipment` |
| Interoperability | Real Yamcs instance decodes packets with generated XTCE; Open MCT plugin displays; FHIR validator accepts bundles; DICOM files open in reference viewer | containers in CI |
| Hardware-in-the-loop | Real BLE devices (pulse oximeter, BP cuff, thermometer, HR strap, GHS device) on a test bench | bench runbook, nightly |
| Fault injection | Device disconnects, corrupt packets, clock jumps, disk full, DB corruption, primary crash, LOS 30 days | scripted scenarios |
| Soak | 14 days continuous, 6 crew, full device load | metrics dashboard |
| UI | Component tests, accessibility audit (axe), keyboard-only, 200 % text, glove mode | Vitest, Playwright |
| Usability | Formative tests, then summative evaluation (IEC 62366-1) with representative users | protocol in `docs/usability/` |
| Clinical review | Flight-surgeon review of ruleset and protocols; analog-mission trial | sign-off records |

`caelum-test-equipment` provides device emulators that speak the same protocols as real devices
(BLE via a virtual adapter or loopback transport, DICOM sender, cFS packet source, Yamcs sink). It
is test equipment, not a product mode: release builds fail CI if the feature is enabled.

## 2. Performance budgets (Windows reference laptop, 6 crew, full device load)
| Metric | Budget |
|---|---|
| Sample → UI latency | ≤ 500 ms p99 |
| Persistence met → alarm on all stations | ≤ 1 s p99 |
| CPU (steady) | ≤ 15 % of 4 cores |
| RAM | ≤ 600 MB total (core + WebView) |
| Disk | ≤ 300 MB per crew member per day before retention |
| Start-up to monitoring | ≤ 10 s |
| Fail-over | ≤ 10 s |

## 3. Traceability
`docs/trace.csv`: requirement ID → feature ID → code module → test names → result. Generated
by a script that scans test names and doc comments for IDs; CI fails if an M-priority feature
has no passing test.

## 4. CI pipeline
fmt, clippy (`-D warnings`), nextest, coverage gates (alarms 100 % branches; core ≥ 85 % lines),
UI typecheck and tests, Playwright, cargo-deny, cargo-audit, pnpm audit, SBOM, release-feature
guard (no `test-equipment`), signed installers for Windows/Linux/macOS.
