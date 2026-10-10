# Third-party software (SOUP) risk register

IEC 62304 SOUP register for dependencies whose maturity or behaviour needs a recorded risk and
mitigation. The full dependency list with exact versions is the CycloneDX SBOM that CI produces;
this register covers the dependencies that need more than an SBOM line, plus every supporting
crate added under the CR-003 dependency policy (permissive licence passing cargo-deny, actively
maintained, no network access or native downloads at build time; a CR is still needed for crates
replacing a stack item or used in `caelum-alarms`, `caelum-ingest`, `caelum-security`,
`caelum-cluster`, crypto or network code).

| Name | Version | Purpose | Risk | Mitigation | Added |
|---|---|---|---|---|---|
| `tauri-specta` | `=2.0.0-rc.25` (newest on crates.io, 2026-05-08); confirmed working with `tauri` 2.12.2 in M0 | Generates typed TypeScript bindings for IPC commands (CLAUDE.md stack) | v2 has only been published as release candidates; the API may change between them; release cadence is irregular (rc.21 January 2025, rc.24 March 2026). Its `invoke_handler` borrows the builder, so the builder is held in a process-wide static | Exact pin; used only in `src-tauri/src/ipc.rs`, so it can be replaced; generated bindings (`ui/src/bindings.ts`) committed and checked for drift by a test; upgrades only as a reviewed change with the bindings diff | M0 |
| `specta`, `specta-typescript` | `=2.0.0-rc.25`, `=0.0.12` | Type reflection and TypeScript export used by `tauri-specta` (version pairs fixed by it) | Same release-candidate risk as `tauri-specta`; `specta-typescript` is pre-1.0 | Exact pins; used only in `src-tauri` (`ipc.rs` and command types); same drift test | M0 |
| `serde_json` | `1` (`Cargo.lock`) | Dev-dependency of `caelum-desktop`: configuration test parses `tauri.conf.json` and capabilities | Test-only; low. MIT/Apache-2.0; actively maintained | Dev-dependency only | M0 |
| `tauri` (stack item) | `2.12.2` with default features except `common-controls-v6` | Application shell | That default feature makes Windows test binaries fail to load (`STATUS_ENTRYPOINT_NOT_FOUND`); it only serves the predefined `about` menu item, which CAELUM does not use | Disabled in `src-tauri/Cargo.toml`; re-enabling it needs the test-manifest fix and a review | M0 |
| `cargo_metadata` | `0.23.1` (`Cargo.lock`) | Dev-dependency of `caelum-core`: architecture test reads the workspace layout (`cargo metadata --no-deps`, offline) | Test-only; low. MIT licence; actively maintained | Dev-dependency only, never in a release build; no network or native downloads at build time | M0 |
| `cssparser`, `cssparser-macros`, `selectors`, `dtoa-short`, `option-ext` (via `tauri`, `dirs`) | `Cargo.lock` | HTML/CSS handling in Tauri's build tooling and config; user directories | Licensed MPL-2.0 (file-level copyleft), outside the permissive allow-list | Allowed per crate in `deny.toml` (any new MPL crate fails until reviewed); used unmodified; obligation: state where the MPL source is available, carried into NOTICE with the release SBOM | M0 |

| `uuid` | `1.28` (`Cargo.lock`) | Typed identifiers in `caelum-domain` (parse and format only; generation happens in the storage and security crates) | Low. MIT/Apache-2.0; actively maintained; no network or native downloads | Used without random-generation features in the domain crate | M1 |
| `proptest` | `1.11` (`Cargo.lock`) | Property tests (CLAUDE.md stack) for units and time conversions | Test-only; low | Dev-dependency only | M1 |
| `rusqlite` (stack) with `bundled-sqlcipher-vendored-openssl`; bundled SQLCipher, OpenSSL | `0.40.2`; `libsqlite3-sys 0.38.2` = SQLite 3.51.3 / SQLCipher 4.14.0; `openssl-src 300.6.1+3.6.3` = OpenSSL 3.6.3 | Encrypted onboard database (F-SYS-02, R-SEC-01) | OpenSSL advisories arrive through `openssl-src` releases; a plain SQLite build would store data unencrypted; the build needs Perl (Strawberry Perl on Windows) | Versions fixed in `Cargo.lock`; `Database::open` refuses to run without SQLCipher (`cipher_version` check); gate test proves the file has no SQLite header and no plaintext; `cargo audit` covers `openssl-src` | M1 |
| `refinery` | `0.10` (`Cargo.lock`) | Schema migrations embedded at compile time (CLAUDE.md stack) | 0.10 supports `rusqlite` only up to 0.40, which holds back `rusqlite` upgrades | Migrations are plain SQL in `crates/caelum-store/migrations/`; a test checks the tables against docs/05 §2 | M1 |
| `serialport` (stack) | `4.10` (`Cargo.lock`), default features off (no `libudev`) | USB/serial device framework (F-ACQ-04); ports are opened read-only (no write handle leaves the module) | Licensed MPL-2.0 (file-level copyleft), outside the permissive allow-list; without `libudev`, Linux port enumeration gives less USB detail | Allowed per crate in `deny.toml`; used unmodified; MPL source location carried into NOTICE with the release SBOM; serial writes (poll requests) only through the write-exception register | M2 |
| `btleplug` (stack) | `0.13.4` (`Cargo.lock`) | BLE central on WinRT, BlueZ and CoreBluetooth (F-ACQ-01/02) | Pre-1.0; behaviour differs per platform (RSSI updates, pairing); no pairing API (CR-002 B7) | Used only in `caelum-devices::ble`; the single device write (CCCD enable) is WX-001; reconnection policy is CAELUM's own, tested without the radio (`link`); platform pairing modules (`pairing`) | M2 |
| `bluer` (stack) | `0.17` (`Cargo.lock`), Linux only | BlueZ pairing (F-ACQ-01) | Needs a running `bluetoothd` and D-Bus | Used only in `caelum-devices::pairing`; failures fall back to guided OS pairing | M2 |
| `hidapi` | `2.6.7` (`Cargo.lock`), backends `windows-native` and `linux-native-basic-udev` | HID input reports (F-ACQ-04) | Pre-built C library on macOS only; the device handle could also write | Pure-Rust backends on Windows and Linux (no C build, no libudev); handles are wrapped as read-only (`hid::ReadOnlyHid`); MIT licence | M2 |
| `toml` | `1` (`Cargo.lock`) | Parsing the signed alarm ruleset (CR-009) and signed configuration bundles (crew roster, CR-012) | A parser bug could mis-read a rule | Unknown fields refused; the whole ruleset is validated after parsing and rejected on any error; the bytes are verified by digest and signature before parsing (SR-009); a test checks every rule against docs/06 §3 | M3 |
| `fs4` | `1.1` (`Cargo.lock`) | Free disk space for the storage self-test and watchdog (F-SYS-09, CR-011) | Low; pure Rust, MIT OR Apache-2.0 | Read-only query; a failure is reported as a failed check (never as enough space) | M4 |
| `axe-core` (npm, dev only) | `4.14.0` | Accessibility checks in the UI tests (M5 gate) | Licensed MPL-2.0; test-only, never shipped (the npm licence guard covers shipped packages) | Dev dependency only; used unmodified | M5 |
| `playwright`, `@vitest/browser-playwright` (npm, dev only) | `1.64.0`, `5.0.3` | Real-browser test runs for the axe checks and screenshots (CLAUDE.md stack: Vitest browser mode with Playwright) | Downloads a Chromium build at test time (online CI and developer machines); not part of offline release builds | Dev dependencies only; the offline release build does not run UI tests | M5 |
| Security crates (`argon2`, `ed25519-dalek`, `sha2`, `getrandom`, `keyring-core` and the platform keyring stores) | see CR-006 | Passwords, audit signatures and hash chain, randomness, OS keystore | Recorded in CR-006 §1; `keyring-core` is a new major line | Wrapped behind CAELUM traits in `caelum-security`; parameters fixed in CR-006 §2 | M1 |

## Bundled runtime components
| Name | Version | Purpose | Risk | Mitigation | Added |
|---|---|---|---|---|---|
| Microsoft WebView2 Fixed Version runtime (x64) | `155.0.4283.45` (`tools/ci/src/webview2.ts`) | Browser engine that renders the CAELUM UI on Windows (CR-004) | Does not update itself: security fixes reach the vehicle only through CAELUM releases; Microsoft supports a Fixed Version only for a limited time; no published digest; about 670 MB installed | Pinned and tested with the app; package accepted only with a valid Microsoft Corporation Authenticode signature, then held in the checksummed tool cache; refresh at least each release and whenever Microsoft publishes a security fix; per-machine install keeps the sandbox permissions standard | M0 |

## Accepted security advisories
Each ID is ignored in both `deny.toml` and `.cargo/audit.toml` (kept equal by
`tools/ci/test/supply-chain.test.ts`) and re-checked on every Tauri upgrade. None can be fixed
inside the locked stack.

| Advisory | Crate | Kind | Where it enters | Why accepted | Added |
|---|---|---|---|---|---|
| RUSTSEC-2024-0429 | `glib` 0.18 | Unsound `VariantStrIter` iterator impls | Linux only: Tauri 2 GTK backend (`gtk`, `wry`, `tao`) | CAELUM never uses that API; nothing of it on Windows; the fix needs gtk-rs ≥ 0.20, which Tauri 2 has not adopted | M0 |
| RUSTSEC-2024-0370 | `proc-macro-error` | Unmaintained | Linux only, compile time: `glib-macros` | Procedural macro, no runtime code; no known vulnerability | M0 |
| RUSTSEC-2024-0436 | `paste` | Unmaintained | Compile time: `specta` (via `tauri-specta`) | Procedural macro, no runtime code; no known vulnerability | M0 |

## Known risks to enter when the dependency is added
- `cfdp-rs` 0.3 (M10): no inactivity handling, suspend, keep-alive/prompt or
  transmission-opportunity handling. Mitigation: the store-and-forward queue handles loss of
  signal and suspends and resumes transactions; missing pieces are built on top (CR-002 B8).
- `btleplug` (M2): no pairing or bonding API. Mitigation: platform pairing modules (CR-002 B7).
- `ort` / ONNX Runtime (M12): downloads binaries at build time by default. Mitigation:
  `load-dynamic` with a pinned, checksummed binary from the tool cache (CR-002 D4).
- `rusqlite` with `bundled-sqlcipher-vendored-openssl` (M1): recorded in the table above
  (SQLCipher 4.14.0, OpenSSL 3.6.3).
