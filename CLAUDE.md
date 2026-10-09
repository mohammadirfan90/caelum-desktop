# CLAUDE.md: CAELUM Desktop

CAELUM is an onboard astronaut health monitoring system: a Tauri 2 desktop application with a
Rust core that connects medical and wearable devices, evaluates crew health with deterministic
safety rules and validated models, escalates alarms to crew and ground, and exchanges data with
mission control using CCSDS, XTCE, Yamcs, Open MCT and HL7 FHIR. It writes the health record to the
onboard medical data system (MDA-class) over FHIR (path A) and sends operational status and alarms
directly on the cFS Software Bus (path B); alarms never depend on the medical data system.

Read before any work: `docs/01_FEATURE_LOCK.md` (scope), `docs/06_SAFETY_AND_ALARMS.md`
(safety), and the current milestone in `docs/10_BUILD_PLAN.md`.

## Non-negotiable rules
1. **Scope is locked.** Build only features listed in `docs/01_FEATURE_LOCK.md`. If something
   seems missing, stop and write a change request in `docs/change-requests/`; do not add it.
2. **No fabricated data, ever.** No placeholder numbers, no random values, no "nominal" defaults.
   A missing or stale value is shown as NO DATA / STALE / SIGNAL UNUSABLE.
3. **No demo mode in release builds.** Device emulators exist only as test equipment behind the
   Cargo feature `test-equipment`, which release builds must not enable (CI checks this).
4. **Safety rules are deterministic and versioned.** Absolute limits are never suppressed by
   baselines or ML. Every rule change bumps `RULESET_VERSION`, updates the hazard log and adds tests.
5. **Read-only to the vehicle and to therapeutic devices.** No code path may send commands to
   spacecraft systems, defibrillators, infusion devices or any actuator. Enforced by crate
   boundaries: `caelum-vehicle` and device adapters expose no write APIs.
6. **Every IPC command and network endpoint is authenticated and authorised** by role and crew
   scope. Mission Control never receives raw physiology.
7. **One source of truth:** parameters in `crates/caelum-domain/src/parameters.rs`; alarm rules in
   `crates/caelum-alarms/rules/*.toml` (signed); UI tokens in `ui/src/styles/tokens.css`.
8. **Offline first.** The app must build, install and run with no internet access.
9. **Traceability.** Every requirement ID (e.g. `SR-012`, `F-ALM-03`) that a change implements
   is cited in the commit message and in a test name or doc comment.

## Stack (do not substitute without a change request)
- Shell: Tauri 2 (`tauri`, `tauri-plugin-*` v2), WebView UI.
- UI: React 19 + TypeScript (strict) + Vite + Tailwind CSS v4, design tokens from the prototype.
  Type-safe IPC bindings generated with `tauri-specta` (no hand-written command types).
- Core: Rust stable, edition 2021, `tokio`, `serde`, `thiserror`, `tracing`.
- Storage: SQLite via `rusqlite` with `bundled-sqlcipher` (encrypted at rest), `refinery` migrations.
- Devices: `btleplug` (BLE), `serialport` (USB/serial), `dicom` (dicom-rs), `quick-xml`.
- Space comms: `spacepackets` (CCSDS 133.0-B-2, CUC/CDS time), `cfdp-rs` (CCSDS 727.0-B-5).
- Network: `axum` + `tokio-tungstenite` for the crew LAN and ground gateway, `rustls` for TLS.
- Crypto: `argon2` (passwords), `ed25519-dalek` (signing), `aes-gcm` via SQLCipher keys, `zeroize`.
- ML: `ort` (ONNX Runtime) with models pinned by hash.
- Tests: `cargo nextest`, `proptest`, `insta`, Playwright for UI, `cargo-llvm-cov`.
- Supply chain: `cargo-deny`, `cargo-audit`, `pnpm audit`, SBOM (CycloneDX) per release.

## Commands
```bash
pnpm install                       # UI dependencies
cargo nextest run --workspace      # all Rust tests
cargo clippy --workspace -- -D warnings
pnpm -C ui typecheck && pnpm -C ui test
pnpm tauri dev                     # run the desktop app
pnpm tauri build                   # release installers (no test-equipment feature)
cargo deny check && cargo audit
```

## Definition of done (every task)
- Tests added for the behaviour, including failure paths; all tests and clippy pass.
- No `unwrap()`/`expect()` in non-test code outside `main` initialisation; no `unsafe` without a
  `SAFETY:` comment and review note.
- Requirement IDs cited; `docs/` updated if behaviour or interfaces changed.
- UI: keyboard operable, works at 200 % text size and in glove mode, no colour-only meaning.
- Performance budgets in `docs/09_VV_PLAN.md` still met.
