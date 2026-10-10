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
- `rusqlite` with `bundled-sqlcipher-vendored-openssl` (M1): builds SQLCipher and OpenSSL from
  source; needs Strawberry Perl on Windows. Record the OpenSSL version and track its advisories.
