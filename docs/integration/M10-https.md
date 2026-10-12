# M10 integration: HTTPS transport for the medical data system

`caelum_mds::https::HttpsTransport` is the production `FhirTransport` (F-GND-12, R-INT-05,
ICD-MDS §3–4, §6, §9; decision in CR-016 §9). It is complete and tested in isolation against a
loopback TLS 1.3 server with mutual authentication (`crates/caelum-mds/tests/https.rs`). This page
lists what the lead wires and the register rows to add. Nothing outside `caelum-mds` was changed.

## 1. Constructing the transport (core)
```rust
let transport = HttpsTransport::new(HttpsConfig {
    base_url,         // signed configuration
    ca_pem,           // signed configuration: mission CA bundle (PEM)
    client_cert_pem,  // instance identity, provisioned at commissioning
    client_key_pem,   // KeyStore::get(..) -> Zeroizing<Vec<u8>>
    timeout,          // signed configuration
})?;                  // HttpsConfigError: refuse to start the MDS link, never fall back
```
- **Signed configuration.** `base_url`, `ca_pem` and `timeout` come from the signed `ground`
  bundle (`GroundConfig::load_signed`), in a new `[mds]` section next to the MDS
  `ConnectorConfig` (M10.md §2, "Configuration"): `base_url` (string, `https://` only),
  `ca_pem` (string, one or more certificates), `timeout_ms` (integer > 0). No defaults: a missing
  section means MDS `NotConfigured` (routing then follows the mission policy; alarms are on path B
  regardless, SR-015). A missing or invalid field refuses the bundle.
- **Instance identity (ICD-MDS §8: machine identity per instance).** The client certificate and
  private key are per installation, so they are not in the mission-wide bundle. Proposed:
  keystore entries `mds-client-key` (PEM private key) and `mds-client-cert` (PEM chain, leaf
  first) in `caelum-security::keystore` (OS credential store, never a file), written at
  commissioning by an Administrator after re-authentication and audited (`mds.identity.set`).
  `KeyStore::get` already returns `Zeroizing<Vec<u8>>`, which is the type of
  `HttpsConfig::client_key_pem`. Certificate rotation = write the new pair, rebuild the
  transport.
- **Threading.** The transport is blocking. Run `MdsConnector::tick(now, &mut transport)` on a
  dedicated thread or in `tokio::task::spawn_blocking`, never on the alarm path or a runtime
  worker. Worst case per tick: (creates sent per tick + `SEARCH_PAGE_BUDGET`) × `timeout`, so
  choose `timeout` with the poll interval (ICD-MDS §6: 10 s polling; alarm-related resources
  within 5 s) in mind; a few seconds is appropriate on the crew LAN.
- **Health.** No extra wiring: `MdsConnector` records every transport result; `Unreachable`,
  `Timeout`, `Http5xx` and `Protocol` do not prove contact, so a misconfigured CA, a wrong
  certificate or a server outside the base URL shows as the MDS link being down and raises
  `technical.mds_unreachable` after the configured limit.
- **Logging.** `HttpsConfig`'s `Debug` prints sizes only, never the key. `TransportError` texts
  can contain server `OperationOutcome` diagnostics (held-for-review reason); they stay on the
  medical path and must not go on the OPS channel (rule 6), which the `OpsRecord` types already
  prevent.

## 2. Behaviour the lead can rely on
| Situation | Result |
|---|---|
| TCP refused / host not found / TLS failure (wrong CA, wrong server name, TLS 1.2 only, client certificate rejected) | `Unreachable` (retried) |
| No complete response within `timeout` | `Timeout` (retried; conditional create keeps it idempotent) |
| 201 / 200 on create | `created = true / false`; id from the body, else `Location` |
| 4xx | `Http4xx { status, outcome }`, outcome = `OperationOutcome` diagnostics or the reason phrase, ≤ 1024 chars (held, except 408 / 429) |
| 5xx | `Http5xx` (retried) |
| 3xx, other 2xx, invalid JSON, not a searchset `Bundle`, entry of another type, body > 16 MiB, `next` link outside the base URL | `Protocol` (retried) |
| Malformed resource type, `resourceType` ≠ endpoint, `_count = 0` | `Http4xx { 400, "not sent: …" }`, nothing sent |
| Search paging | `next` links followed up to `SEARCH_PAGE_BUDGET` = 8 pages per call; the connector's immediate re-poll after a full page continues from the new high-water mark |

No proxy is used (proxy environment variables are ignored), redirects are never followed, the
public web roots are never trusted and verification cannot be disabled.

## 3. HAPI FHIR CI job (M10.md §6 item 2, still open)
HAPI must present a server certificate issued by the test mission CA for the host name used in
`base_url` and require client certificates (e.g. a TLS-terminating proxy in front of
`hapiproject/hapi:v7` with `ssl_verify_client on`, TLS 1.3 only). Configure HAPI's server address
(`hapi.fhir.server_address`) to the same external `base_url`, otherwise its `next` links point at
the internal address and are refused as outside the base URL.

## 4. Rows for `docs/third-party-risk.md`
| Name | Version | Purpose | Risk | Mitigation | Added |
|---|---|---|---|---|---|
| `ureq` | `3.3.0`, `default-features = false`, `rustls-no-provider` | Blocking HTTPS client for the MDS FHIR transport (F-GND-12, R-INT-05; CR-016 §9). MIT OR Apache-2.0 | Single-maintainer project; the rustls integration API it exposes is explicitly "unversioned" (may change in a minor release); internally `expect`s on a client certificate rustls refuses | Used only in `caelum_mds::https`; version fixed in `Cargo.lock`; the full rustls configuration is pre-built and validated in `HttpsTransport::new`, so the internal `expect` cannot be reached with accepted input; behaviour pinned by the loopback mTLS tests | M10 |
| `ureq-proto` | `0.6.0` | HTTP/1.1 state machine of `ureq`. MIT OR Apache-2.0 | Parser of untrusted server responses | Response header size and body size bounded; covered by `cargo audit` | M10 |
| `utf8-zero` | `0.8.1` | UTF-8 decoding in `ureq`. MIT OR Apache-2.0 | Small, low activity | Transitive only; `cargo audit` | M10 |
| `rustls` (stack) | `0.23.45`, `default-features = false`, `ring`, `std` (`ureq` adds `tls12`, `logging`) | TLS 1.3 with mutual certificates to the MDS. Apache-2.0 OR ISC OR MIT | TLS implementation bugs; `tls12` code is compiled in (enabled by `ureq`) | Only TLS 1.3 suites are offered (provider filtered; a TLS 1.2-only server is refused in a test); mission CA only; `cargo audit` | M10 |
| `rustls-webpki` | `0.103.15` | Certificate path and name verification. **ISC** | Verifier bugs | `cargo audit`; tests for wrong CA, wrong name and foreign client CA | M10 |
| `rustls-pki-types` | `1.15.1` | Certificate / key types and PEM parsing. MIT OR Apache-2.0 | Low | Transitive | M10 |
| `ring` | `0.17.14` | Crypto provider for rustls (no aws-lc-rs / cmake). **Apache-2.0 AND ISC** | Contains C and assembly built by `cc` at build time (needs the C toolchain already required for SQLCipher; no download); maintenance was handed over in 2025 (now maintained under the rustls project) | Version fixed in `Cargo.lock`; `cargo audit`; offline build from the vendored crate | M10 |
| `untrusted` | `0.9.0` | Safe parsing helper of `ring` / `webpki`. **ISC** | Low activity (stable) | Transitive | M10 |
| `zeroize` (stack) | `1.9.1` | Wipes the client key PEM on drop | Low | Already in the workspace | M10 |
| `rcgen` (dev only) | `0.14.10`, `default-features = false`, `pem`, `ring` | Test certificates for the loopback TLS server. MIT OR Apache-2.0 | None in release (dev-dependency); pulls `x509-parser` / `yasna` / `time` into the lockfile only | Not linked into any shipped binary | M10 |

## 5. Licence note: deny.toml
`ring` (`Apache-2.0 AND ISC`), `rustls-webpki` (`ISC`) and `untrusted` (`ISC`) are under ISC, which
is **not** in the `deny.toml` allow-list, so `cargo deny check` will fail until it is resolved.
ISC is a short OSI-approved permissive licence equivalent to MIT; the `rustls` stack item already
implies these crates. Recommended: add `"ISC"` to `[licenses] allow` (or, narrower, crate
exceptions `{ allow = ["ISC"], crate = "ring" }`, `"rustls-webpki"`, `"untrusted"`), and carry
the ISC notices into `NOTICE` with the release SBOM. All other new crates are MIT and / or
Apache-2.0. `Cargo.lock` gained the entries above (it was updated offline from the local registry
cache).
