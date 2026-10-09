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
| `tauri-specta` | `2.0.0-rc.25`, exact `=` pin (newest on crates.io, 2026-05-08); compatibility confirmed when it is added in M0 | Generates typed TypeScript bindings for IPC commands (CLAUDE.md stack) | v2 has only been published as release candidates; the API may change between them; release cadence is irregular (rc.21 January 2025, rc.24 March 2026) | Exact pin; used only through a thin wrapper module in `src-tauri`, so it can be replaced; generated bindings committed and checked for drift in CI; upgrades only as a reviewed change with the bindings diff | M0 |

## Known risks to enter when the dependency is added
- `cfdp-rs` 0.3 (M10): no inactivity handling, suspend, keep-alive/prompt or
  transmission-opportunity handling. Mitigation: the store-and-forward queue handles loss of
  signal and suspends and resumes transactions; missing pieces are built on top (CR-002 B8).
- `btleplug` (M2): no pairing or bonding API. Mitigation: platform pairing modules (CR-002 B7).
- `ort` / ONNX Runtime (M12): downloads binaries at build time by default. Mitigation:
  `load-dynamic` with a pinned, checksummed binary from the tool cache (CR-002 D4).
- `rusqlite` with `bundled-sqlcipher-vendored-openssl` (M1): builds SQLCipher and OpenSSL from
  source; needs Strawberry Perl on Windows. Record the OpenSSL version and track its advisories.
