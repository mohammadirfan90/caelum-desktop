# CLAUDE.md: CAELUM Desktop

CAELUM is an onboard astronaut health monitoring system: a Tauri 2 desktop application with a
Rust core that connects medical and wearable devices, evaluates crew health with deterministic
safety rules and validated models, escalates alarms to crew and ground, and exchanges data with
mission control using CCSDS, XTCE, Yamcs, Open MCT and HL7 FHIR. It writes the health record to the
onboard medical data system (MDA-class) over FHIR (path A) and sends operational status and alarms
directly to the vehicle as CCSDS telemetry (path B; entry into cFS per `docs/icd/ICD-VEH.md`);
alarms never depend on the medical data system.

Read before any work: `docs/01_FEATURE_LOCK.md` (scope), `docs/06_SAFETY_AND_ALARMS.md`
(safety), and the current milestone in `docs/10_BUILD_PLAN.md`.
Project state, decisions, open items and how to resume: `docs/PROGRESS.md`; update it with every
push, so each push carries its context.

## Non-negotiable rules
1. **Scope is locked.** Build only features listed in `docs/01_FEATURE_LOCK.md`. If something
   seems missing, stop and write a change request in `docs/change-requests/`; do not add it.
2. **No fabricated data, ever.** No placeholder numbers, no random values, no "nominal" defaults.
   A missing or stale value is shown as NO DATA / STALE / SIGNAL UNUSABLE. Missing context is
   `unknown`, never a default (SR-018). Readings a device flags as demonstration or test data are
   rejected in release builds (SR-017).
3. **No demo mode in release builds.** Device emulators exist only as test equipment behind the
   Cargo feature `test-equipment`, which release builds must not enable (CI checks this).
4. **Safety rules are deterministic and versioned.** Absolute limits are never suppressed by
   baselines or ML. Every rule change bumps `RULESET_VERSION`, updates the hazard log and adds tests.
5. **Read-only to the vehicle and to therapeutic devices.** No code path may send commands to
   spacecraft systems, defibrillators, infusion devices or any actuator. Enforced by crate
   boundaries and CI: `caelum-vehicle` has no write API; CAELUM status telemetry is built by
   `caelum-ground` with a telemetry-only packet type; device adapters write only through entries
   in the reviewed register `docs/write-exceptions.md` (SIG acquisition control writes, serial
   poll requests defined in a device ICD, wristband Immediate Alert). The CI guard ships with zero
   exceptions; never add a write without a reviewed register entry.
6. **Every IPC command and network endpoint is authenticated and authorised** by role and crew
   scope. Mission Control (OPS channel) never receives physiology: no measured value, parameter
   name, rule id or free text.
7. **One source of truth:** parameters in `crates/caelum-domain/src/parameters.rs`; alarm rules in
   `crates/caelum-alarms/rules/*.toml` (signed); UI tokens in `ui/src/styles/tokens.css`; device
   writes in `docs/write-exceptions.md`; third-party risks in `docs/third-party-risk.md`.
8. **Offline first.** The app must install, run and update with no internet access; release builds
   are reproducible offline from the prepared, checksummed tool cache (`docs/OFFLINE_BUILD.md`).
9. **Traceability.** Every requirement ID (e.g. `SR-012`, `F-ALM-03`) that a change implements
   is cited in the commit message and in a test name or doc comment.

Licence: Apache-2.0 (`LICENSE`, `NOTICE`), copyright ChronoStrider. Bundle identifier
`org.chronostrider.caelum` (changing it needs a CR). Version starts at 0.1.0; 1.0.0 = every M-priority
feature complete and gated. Clinical content is DRAFT under a provisional medical lead (CR-003).

## Stack (do not substitute without a change request)
Supporting crates not listed below may be added without a CR only if they have a permissive
licence that passes cargo-deny, are actively maintained, do no network access or native downloads
at build time, and are recorded in `docs/third-party-risk.md`. A CR is required for any crate that
replaces a stack item, or that is used in `caelum-alarms`, `caelum-ingest`, `caelum-security`,
`caelum-cluster`, crypto code or network code (CR-003 §5).
- Shell: Tauri 2 (`tauri`, `tauri-plugin-*` v2), WebView UI.
- UI: React 19 + TypeScript (strict) + Vite + Tailwind CSS v4, design tokens from the prototype.
  Type-safe IPC bindings generated with `tauri-specta` (no hand-written command types), pinned to
  an exact v2 release candidate and used only through a thin wrapper (see `docs/third-party-risk.md`).
- Core: Rust stable, edition 2021, `tokio`, `serde`, `thiserror`, `tracing`.
- Storage: SQLite via `rusqlite` with `bundled-sqlcipher` (SQLCipher 4: AES-256-CBC with
  HMAC-SHA512 page authentication), `refinery` migrations.
- Devices: `btleplug` (BLE), `windows` (WinRT pairing) and `bluer` (BlueZ pairing), `serialport`
  (USB/serial), `dicom` (dicom-rs), `quick-xml`.
- Space comms: `spacepackets` (CCSDS 133.0-B-2, CUC/CDS time), `cfdp-rs` (CCSDS 727.0-B-5).
- Network: `axum` + `tokio-tungstenite` for the crew LAN and ground gateway, `rustls` for TLS.
- Crypto: `argon2` (passwords), `ed25519-dalek` (signing), `aes-gcm` (attachment file store),
  `zeroize`.
- ML: `ort` (ONNX Runtime, `load-dynamic` with a pinned, checksummed runtime binary) with models
  pinned by hash.
- Tests: `cargo nextest`, `proptest`, `insta`, `cargo-llvm-cov` (stable gates: `caelum-alarms`
  ≥ 95 % region, other core crates ≥ 85 % line; nightly branch-coverage report for
  `caelum-alarms`, informational). UI: Vitest (browser mode with Playwright for accessibility);
  end-to-end with `tauri-driver` + WebdriverIO on Windows and Linux.
- Supply chain: `cargo-deny`, `cargo-audit`, `pnpm audit` (online in CI; dated advisory snapshot
  for air-gapped builds), SBOM (CycloneDX) per release.

## Commands
```bash
pnpm install                       # UI dependencies
cargo nextest run --workspace      # all Rust tests
cargo nextest run -p caelum-store -p caelum-ingest -p caelum-core --features test-equipment  # test-equipment-only tests
cargo clippy --workspace -- -D warnings
pnpm -C ui typecheck && pnpm -C ui test
pnpm tauri dev                     # run the desktop app
pnpm tauri build                   # development installers (system WebView2; never shipped)
node tools/offline/install-webview2.ts --cache <dir>   # Windows release: pinned WebView2 runtime (CR-004)
pnpm tauri build --config src-tauri/tauri.release.conf.json   # release installers (no test-equipment)
cargo deny check && cargo audit --deny warnings && pnpm audit   # online
pnpm -C tools/ci check             # release-feature, write-exception and npm licence guards
pnpm -C tools/ci test              # guard self-tests
pnpm -C tools/ci sbom              # CycloneDX SBOMs into target/sbom/
pnpm -C tools/ci trace             # run all suites, write docs/trace.csv, enforce complete milestones
pnpm -C tools/ci check-all         # every check above in CI order (add -- --offline when air-gapped)
node tools/offline/verify.ts --cache <dir>         # tool cache intact (docs/OFFLINE_BUILD.md)
cargo run -p caelum-ruleset-signer -- sign --ruleset crates/caelum-alarms/rules/ruleset.toml --key <key outside repo> --role author   # re-sign after a ruleset change (CR-009)
cargo run -p caelum-ruleset-signer -- verify --ruleset crates/caelum-alarms/rules/ruleset.toml [--policy flight]
node tools/offline/build-offline.ts --cache <dir>  # release build from the tool cache only
```

## Definition of done (every task)
- Tests added for the behaviour, including failure paths; all tests and clippy pass.
- No `unwrap()`/`expect()` in non-test code outside `main` initialisation; no `unsafe` without a
  `SAFETY:` comment and review note.
- Requirement IDs cited; `docs/` updated if behaviour or interfaces changed.
- Every device write has a reviewed entry in `docs/write-exceptions.md`; every new dependency with
  a known maturity risk has an entry in `docs/third-party-risk.md`.
- UI: keyboard operable, works at 200 % text size and in glove mode, no colour-only meaning; text
  ≥ 4.5:1 and UI elements ≥ 3:1 contrast in every theme (automated contrast test).
- Performance budgets in `docs/09_VV_PLAN.md` still met.
