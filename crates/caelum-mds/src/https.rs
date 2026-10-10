//! Production FHIR transport: HTTPS over TLS 1.3 with mutual certificates (F-GND-12, R-INT-05,
//! ICD-MDS §3–4, §6, §9; CR-016 "HTTPS transport").
//!
//! - **TLS:** rustls with the `ring` provider restricted to the TLS 1.3 cipher suites, so a
//!   server that offers only TLS 1.2 is refused. The server is verified against the mission CA
//!   bundle from signed configuration only (never the public web roots) with server name
//!   verification on; the client presents its machine certificate (ICD-MDS §8). There is no
//!   option to disable verification.
//! - **HTTP:** blocking HTTP/1.1 (`ureq`), no proxy (environment variables are ignored), HTTPS
//!   only, no redirects followed, no compression, response bodies capped at
//!   [`MAX_RESPONSE_BYTES`]. One timeout ([`HttpsConfig::timeout`]) bounds each request from
//!   connect to the end of the response body.
//! - **Operations:** only the two of [`FhirTransport`]: conditional create (`POST` with
//!   `If-None-Exist: identifier=urn:caelum:client-id|<id>`) and search (`GET` on
//!   `_lastUpdated`). No update, delete or operation call exists (ICD-MDS §7).
//! - **Errors** are classified for the outbox retry policy (ICD-MDS §9): connect or TLS failure →
//!   [`TransportError::Unreachable`]; timeout → [`TransportError::Timeout`]; 4xx →
//!   [`TransportError::Http4xx`] with the `OperationOutcome` diagnostics; 5xx →
//!   [`TransportError::Http5xx`]; anything else the server sends that is not the expected FHIR
//!   JSON (3xx, unexpected 2xx, bad JSON, wrong bundle) → [`TransportError::Protocol`].
//! - **Paging:** a search follows the bundle's `next` links, up to [`SEARCH_PAGE_BUDGET`] pages
//!   in total, and returns the entries of all pages in order. The connector moves its
//!   `_lastUpdated` high-water mark over the result and polls again at once after a full page, so
//!   the budget only bounds the work per poll; following `next` matters when more than one
//!   page of resources share the same `lastUpdated` instant, which the `ge` cursor alone cannot
//!   step past. A `next` link is followed only when it stays under the configured base URL
//!   (same scheme, host and port, path below the base); otherwise the search fails with
//!   [`TransportError::Protocol`], so the client certificate is never presented elsewhere and a
//!   misconfigured server address is visible as a connection health problem.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{RootCertStore, SupportedCipherSuite};
use serde_json::Value;
use ureq::http::{StatusCode, Uri};
use ureq::tls::{Certificate, ClientCert, PemItem, PrivateKey, RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;
use zeroize::Zeroizing;

use crate::fhir::CLIENT_ID_SYSTEM;
use crate::ids::ClientId;
use crate::transport::{CreateOutcome, FhirTransport, SearchQuery, TransportError};

/// Maximum number of pages one [`FhirTransport::search`] call reads (the first page plus
/// `next` links). See the module documentation.
pub const SEARCH_PAGE_BUDGET: usize = 8;

/// Maximum size of one response body; a larger body is a [`TransportError::Protocol`].
pub const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum length kept of `OperationOutcome` diagnostics in [`TransportError::Http4xx`].
pub const MAX_OUTCOME_CHARS: usize = 1024;

/// FHIR JSON media type (FHIR R4 §3.1.0.1.8).
const FHIR_JSON: &str = "application/fhir+json";

/// Connection settings for the medical data system, from signed configuration (ICD-MDS §3, §8).
/// There are no defaults: every field is mission configuration.
#[derive(Clone)]
pub struct HttpsConfig {
    /// FHIR R4 base URL, `https://host[:port]/path`, without query or fragment.
    pub base_url: String,
    /// Mission CA bundle (one or more PEM certificates) that the server certificate must chain to.
    pub ca_pem: Vec<u8>,
    /// This instance's client certificate chain (PEM), leaf first.
    pub client_cert_pem: Vec<u8>,
    /// The client certificate's private key (PEM: PKCS#8, PKCS#1 or SEC1). Wiped on drop.
    pub client_key_pem: Zeroizing<Vec<u8>>,
    /// Bound on each request, from connect to the end of the response body; must be > 0.
    pub timeout: Duration,
}

impl fmt::Debug for HttpsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpsConfig")
            .field("base_url", &self.base_url)
            .field("ca_pem", &format_args!("{} bytes", self.ca_pem.len()))
            .field(
                "client_cert_pem",
                &format_args!("{} bytes", self.client_cert_pem.len()),
            )
            .field("client_key_pem", &"<redacted>")
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Why an [`HttpsConfig`] was refused. The transport is never built from a partial configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpsConfigError {
    /// The base URL is not an absolute `https://` URL without query, fragment or user info.
    #[error("invalid base URL: {0}")]
    BaseUrl(String),
    /// The CA bundle is empty or holds a certificate that cannot be used as a trust anchor.
    #[error("invalid CA bundle: {0}")]
    CaBundle(String),
    /// The client certificate chain is empty or unreadable.
    #[error("invalid client certificate: {0}")]
    ClientCertificate(String),
    /// The private key is missing, unreadable, or does not match the certificate.
    #[error("invalid client key: {0}")]
    ClientKey(String),
    /// The timeout is zero.
    #[error("timeout must be greater than zero")]
    Timeout,
}

/// The base URL, split for building request URLs and checking `next` links.
#[derive(Debug, Clone)]
struct Base {
    /// The base URL without a trailing `/`.
    url: String,
    scheme: String,
    host: String,
    port: u16,
    /// The base path without a trailing `/` (empty for the root).
    path: String,
}

impl Base {
    fn parse(s: &str) -> Result<Self, HttpsConfigError> {
        let bad = |why: &str| HttpsConfigError::BaseUrl(format!("{s}: {why}"));
        let uri: Uri = s.parse().map_err(|_| bad("not a URL"))?;
        let parts = origin(&uri).ok_or_else(|| bad("not an absolute https URL"))?;
        if parts.0 != "https" {
            return Err(bad("scheme must be https"));
        }
        if uri.query().is_some() || s.contains('#') {
            return Err(bad("query or fragment not allowed"));
        }
        if uri.authority().is_some_and(|a| a.as_str().contains('@')) {
            return Err(bad("user info not allowed"));
        }
        let path = uri.path().trim_end_matches('/').to_owned();
        let url = format!("{}://{}{}", parts.0, authority_text(&uri), path);
        Ok(Self {
            url,
            scheme: parts.0,
            host: parts.1,
            port: parts.2,
            path,
        })
    }

    /// Whether `next` is under this base: same scheme, host and port, path at or below the base.
    fn contains(&self, next: &Uri) -> bool {
        let Some((scheme, host, port)) = origin(next) else {
            return false;
        };
        if (scheme.as_str(), host.as_str(), port)
            != (self.scheme.as_str(), self.host.as_str(), self.port)
        {
            return false;
        }
        let p = next.path();
        match p.strip_prefix(self.path.as_str()) {
            Some(rest) => rest.is_empty() || rest.starts_with('/'),
            None => false,
        }
    }
}

/// (scheme, host, port) of an absolute http(s) URI, lower-cased, with the default port filled in.
fn origin(uri: &Uri) -> Option<(String, String, u16)> {
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let host = uri.host()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let default = match scheme.as_str() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    Some((scheme, host, uri.port_u16().unwrap_or(default)))
}

fn authority_text(uri: &Uri) -> String {
    uri.authority()
        .map(|a| a.as_str().to_owned())
        .unwrap_or_default()
}

/// The `ring` provider restricted to TLS 1.3 cipher suites (ICD-MDS §3).
fn tls13_provider() -> CryptoProvider {
    let mut provider = rustls::crypto::ring::default_provider();
    provider
        .cipher_suites
        .retain(|s| matches!(s, SupportedCipherSuite::Tls13(_)));
    provider
}

fn parse_certs(pem: &[u8]) -> Result<Vec<Certificate<'static>>, String> {
    let mut out = Vec::new();
    for item in ureq::tls::parse_pem(pem) {
        match item.map_err(|e| e.to_string())? {
            PemItem::Certificate(c) => out.push(c),
            PemItem::PrivateKey(_) => {
                return Err("unexpected private key in certificate PEM".into())
            }
            _ => return Err("unexpected PEM item".into()),
        }
    }
    if out.is_empty() {
        return Err("no PEM certificate found".into());
    }
    Ok(out)
}

fn parse_key(pem: &[u8]) -> Result<PrivateKey<'static>, String> {
    let mut keys = Vec::new();
    for item in ureq::tls::parse_pem(pem) {
        match item.map_err(|e| e.to_string())? {
            PemItem::PrivateKey(k) => keys.push(k),
            _ => return Err("unexpected PEM item in key PEM".into()),
        }
    }
    match keys.len() {
        1 => Ok(keys.remove(0)),
        0 => Err("no PEM private key found".into()),
        _ => Err("more than one private key".into()),
    }
}

/// HTTPS (TLS 1.3, mutual certificates) implementation of [`FhirTransport`] (F-GND-12,
/// R-INT-05). Blocking; one connection pool per transport.
pub struct HttpsTransport {
    agent: Agent,
    base: Base,
}

impl fmt::Debug for HttpsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpsTransport")
            .field("base_url", &self.base.url)
            .finish_non_exhaustive()
    }
}

impl HttpsTransport {
    /// Builds the transport, validating the whole configuration first: the base URL, every CA
    /// certificate, the client chain, and that the key loads and matches the certificate. No
    /// connection is made here.
    pub fn new(config: HttpsConfig) -> Result<Self, HttpsConfigError> {
        let base = Base::parse(&config.base_url)?;
        if config.timeout.is_zero() {
            return Err(HttpsConfigError::Timeout);
        }
        let ca = parse_certs(&config.ca_pem).map_err(HttpsConfigError::CaBundle)?;
        let chain =
            parse_certs(&config.client_cert_pem).map_err(HttpsConfigError::ClientCertificate)?;
        let key = parse_key(&config.client_key_pem).map_err(HttpsConfigError::ClientKey)?;
        let key_der = PrivateKeyDer::from_pem_slice(&config.client_key_pem)
            .map_err(|e| HttpsConfigError::ClientKey(e.to_string()))?;

        // Build the same rustls configuration the HTTP client builds, so that every input
        // problem is reported here as an error instead of failing inside the first request.
        let provider = Arc::new(tls13_provider());
        let mut roots = RootCertStore::empty();
        for c in &ca {
            roots
                .add(CertificateDer::from(c.der().to_vec()))
                .map_err(|e| HttpsConfigError::CaBundle(e.to_string()))?;
        }
        let chain_der: Vec<CertificateDer<'static>> = chain
            .iter()
            .map(|c| CertificateDer::from(c.der().to_vec()))
            .collect();
        let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(rustls::ALL_VERSIONS)
            .map_err(|e| HttpsConfigError::CaBundle(e.to_string()))?
            .with_root_certificates(roots);
        builder
            .with_client_auth_cert(chain_der, key_der)
            .map_err(|e| HttpsConfigError::ClientKey(e.to_string()))?;

        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(provider)
            .root_certs(RootCerts::new_with_certs(&ca))
            .client_cert(Some(ClientCert::new_with_certs(&chain, key)))
            .use_sni(true)
            .disable_verification(false)
            .build();
        let agent: Agent = Agent::config_builder()
            .tls_config(tls)
            .https_only(true)
            .proxy(None)
            .http_status_as_error(false)
            .max_redirects(0)
            .save_redirect_history(false)
            .timeout_global(Some(config.timeout))
            .user_agent(concat!("CAELUM/", env!("CARGO_PKG_VERSION")))
            .accept(FHIR_JSON)
            .build()
            .into();
        Ok(Self { agent, base })
    }

    /// The validated base URL (without a trailing `/`).
    pub fn base_url(&self) -> &str {
        &self.base.url
    }

    /// Reads the response body (bounded) and returns it with the status.
    fn finish(
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Reply, TransportError> {
        let mut response = response.map_err(map_error)?;
        let status = response.status();
        let location =
            header(&response, "location").or_else(|| header(&response, "content-location"));
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(map_error)?;
        Ok(Reply {
            status,
            location,
            body,
        })
    }

    fn get_page(&self, url: &str) -> Result<Value, TransportError> {
        let reply = Self::finish(self.agent.get(url).call())?;
        reply.check()?;
        if reply.status != StatusCode::OK {
            return Err(TransportError::Protocol(format!(
                "unexpected status {} for search",
                reply.status.as_u16()
            )));
        }
        reply.json()
    }
}

/// A complete response.
struct Reply {
    status: StatusCode,
    location: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    /// Maps error and redirect statuses (ICD-MDS §9). 2xx passes.
    fn check(&self) -> Result<(), TransportError> {
        let code = self.status.as_u16();
        if self.status.is_client_error() {
            return Err(TransportError::Http4xx {
                status: code,
                outcome: outcome_text(&self.body).unwrap_or_else(|| {
                    self.status
                        .canonical_reason()
                        .map_or_else(|| format!("HTTP {code}"), str::to_owned)
                }),
            });
        }
        if self.status.is_server_error() {
            return Err(TransportError::Http5xx(code));
        }
        if !self.status.is_success() {
            return Err(TransportError::Protocol(format!(
                "unexpected status {code} (redirects are not followed)"
            )));
        }
        Ok(())
    }

    fn json(&self) -> Result<Value, TransportError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| TransportError::Protocol(format!("invalid JSON: {e}")))
    }
}

fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// The diagnostics of an `OperationOutcome` body (each issue's `diagnostics`, else its
/// `details.text`), joined and bounded; `None` if the body is not an `OperationOutcome`.
fn outcome_text(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    if v["resourceType"] != "OperationOutcome" {
        return None;
    }
    let parts: Vec<&str> = v["issue"]
        .as_array()?
        .iter()
        .filter_map(|i| {
            i["diagnostics"]
                .as_str()
                .or_else(|| i["details"]["text"].as_str())
        })
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("; ");
    Some(joined.chars().take(MAX_OUTCOME_CHARS).collect())
}

/// Maps a client error to the retry classes of ICD-MDS §9.
fn map_error(e: ureq::Error) -> TransportError {
    use std::io::ErrorKind;
    match e {
        ureq::Error::Timeout(_) => TransportError::Timeout,
        ureq::Error::Io(io) if matches!(io.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
            TransportError::Timeout
        }
        ureq::Error::Io(io) => TransportError::Unreachable(io.to_string()),
        ureq::Error::HostNotFound => TransportError::Unreachable("host not found".into()),
        ureq::Error::ConnectionFailed => TransportError::Unreachable("connection failed".into()),
        ureq::Error::Tls(s) => TransportError::Unreachable(format!("TLS: {s}")),
        ureq::Error::Rustls(r) => TransportError::Unreachable(format!("TLS: {r}")),
        ureq::Error::BodyExceedsLimit(n) => {
            TransportError::Protocol(format!("response body larger than {n} bytes"))
        }
        other => TransportError::Protocol(other.to_string()),
    }
}

/// A FHIR resource type name as used in a URL path: `[A-Z][A-Za-z]*`, at most 64 characters.
fn valid_resource_type(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphabetic())
}

/// A FHIR logical id: `[A-Za-z0-9\-\.]{1,64}` (FHIR R4 `id` datatype).
fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// A request refused before anything is sent (a caller error). Classified as a 4xx so that the
/// outbox holds the record for review instead of retrying it (ICD-MDS §9); the text says that
/// nothing was sent.
fn refused_locally(why: String) -> TransportError {
    TransportError::Http4xx {
        status: 400,
        outcome: format!("not sent: {why}"),
    }
}

/// The logical id from `.../<type>/<id>[/_history/<vid>]` (a `Location` header).
fn id_from_location(location: &str, resource_type: &str) -> Option<String> {
    let path = location.split(['?', '#']).next()?;
    let segs: Vec<&str> = path.split('/').collect();
    let at = segs.iter().rposition(|s| *s == resource_type)?;
    let id = segs.get(at + 1)?;
    valid_id(id).then(|| (*id).to_owned())
}

/// Percent-encodes a query value (RFC 3986 unreserved characters are kept).
fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl FhirTransport for HttpsTransport {
    fn create_conditional(
        &mut self,
        resource_type: &str,
        client_id: ClientId,
        resource: &Value,
    ) -> Result<CreateOutcome, TransportError> {
        if !valid_resource_type(resource_type) {
            return Err(refused_locally(format!(
                "invalid resource type {resource_type:?}"
            )));
        }
        if resource["resourceType"] != resource_type {
            return Err(refused_locally(
                "resourceType does not match the endpoint".into(),
            ));
        }
        let body = serde_json::to_vec(resource)
            .map_err(|e| refused_locally(format!("resource not serialisable: {e}")))?;
        let url = format!("{}/{resource_type}", self.base.url);
        let condition = format!("identifier={CLIENT_ID_SYSTEM}|{client_id}");
        let reply = Self::finish(
            self.agent
                .post(&url)
                .header("Content-Type", FHIR_JSON)
                .header("If-None-Exist", &condition)
                .header("Prefer", "return=representation")
                .send(&body[..]),
        )?;
        reply.check()?;
        let created = match reply.status {
            StatusCode::CREATED => true,
            StatusCode::OK => false,
            s => {
                return Err(TransportError::Protocol(format!(
                    "unexpected status {} for conditional create",
                    s.as_u16()
                )))
            }
        };
        // The id from the returned resource, else from Location / Content-Location.
        let from_body = serde_json::from_slice::<Value>(&reply.body)
            .ok()
            .filter(|v| v["resourceType"] == resource_type)
            .and_then(|v| v["id"].as_str().map(str::to_owned))
            .filter(|id| valid_id(id));
        let server_id = from_body
            .or_else(|| {
                reply
                    .location
                    .as_deref()
                    .and_then(|l| id_from_location(l, resource_type))
            })
            .ok_or_else(|| {
                TransportError::Protocol(
                    "no logical id in the response body or Location header".into(),
                )
            })?;
        Ok(CreateOutcome { server_id, created })
    }

    fn search(&mut self, query: &SearchQuery) -> Result<Vec<Value>, TransportError> {
        if !valid_resource_type(&query.resource_type) {
            return Err(refused_locally(format!(
                "invalid resource type {:?}",
                query.resource_type
            )));
        }
        if query.count == 0 {
            return Err(refused_locally("_count must be greater than zero".into()));
        }
        let mut url = format!("{}/{}?", self.base.url, query.resource_type);
        if let Some(ge) = &query.last_updated_ge {
            url.push_str(&format!("_lastUpdated=ge{}&", encode_query(ge)));
        }
        url.push_str(&format!("_sort=_lastUpdated&_count={}", query.count));

        let mut out = Vec::new();
        let mut next = Some(url);
        let mut pages = 0;
        while let Some(page_url) = next.take() {
            if pages == SEARCH_PAGE_BUDGET {
                break;
            }
            pages += 1;
            let bundle = self.get_page(&page_url)?;
            next = bundle_page(&bundle, &query.resource_type, &mut out)?;
            if let Some(n) = &next {
                let uri: Uri = n
                    .parse()
                    .map_err(|_| TransportError::Protocol(format!("invalid next link {n:?}")))?;
                if !self.base.contains(&uri) {
                    return Err(TransportError::Protocol(format!(
                        "next link outside the configured base URL: {n}"
                    )));
                }
            }
        }
        Ok(out)
    }
}

/// Appends the matching resources of a searchset bundle page to `out` and returns its `next`
/// link. Entries with `search.mode` `include` or `outcome` are skipped; a match whose type is
/// not the searched type, or an entry without a resource, is a protocol error.
fn bundle_page(
    bundle: &Value,
    resource_type: &str,
    out: &mut Vec<Value>,
) -> Result<Option<String>, TransportError> {
    let bad = |why: &str| Err(TransportError::Protocol(format!("search bundle: {why}")));
    if bundle["resourceType"] != "Bundle" {
        return bad("not a Bundle");
    }
    if bundle["type"] != "searchset" {
        return bad("not a searchset");
    }
    let entries = match &bundle["entry"] {
        Value::Null => &[][..],
        Value::Array(a) => a.as_slice(),
        _ => return bad("entry is not an array"),
    };
    for entry in entries {
        match entry["search"]["mode"].as_str() {
            None | Some("match") => {}
            Some("include" | "outcome") => continue,
            Some(_) => return bad("unknown search mode"),
        }
        let resource = &entry["resource"];
        if !resource.is_object() {
            return bad("entry without a resource");
        }
        if resource["resourceType"] != resource_type {
            return bad("entry of another resource type");
        }
        out.push(resource.clone());
    }
    let mut next = None;
    if let Some(links) = bundle["link"].as_array() {
        for link in links {
            if link["relation"] == "next" {
                match link["url"].as_str() {
                    Some(u) if next.is_none() => next = Some(u.to_owned()),
                    Some(_) => return bad("more than one next link"),
                    None => return bad("next link without url"),
                }
            }
        }
    } else if !bundle["link"].is_null() {
        return bad("link is not an array");
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base_url_validation() {
        let b = Base::parse("https://MDS.example:8443/fhir/").expect("ok");
        assert_eq!(b.url, "https://MDS.example:8443/fhir");
        assert_eq!((b.host.as_str(), b.port), ("mds.example", 8443));
        assert!(b.contains(
            &"https://mds.example:8443/fhir?_getpages=x"
                .parse()
                .expect("uri")
        ));
        assert!(b.contains(
            &"https://mds.example:8443/fhir/Observation?p=2"
                .parse()
                .expect("uri")
        ));
        assert!(!b.contains(&"https://mds.example:8443/fhirx".parse().expect("uri")));
        assert!(!b.contains(&"https://mds.example/fhir".parse().expect("uri")));
        assert!(!b.contains(&"http://mds.example:8443/fhir".parse().expect("uri")));
        assert!(!b.contains(&"https://evil.example:8443/fhir".parse().expect("uri")));
        let root = Base::parse("https://mds.example").expect("ok");
        assert!(root.contains(&"https://mds.example:443/?page=2".parse().expect("uri")));
        for bad in [
            "http://mds.example/fhir",
            "mds.example/fhir",
            "https://mds.example/fhir?x=1",
            "https://mds.example/fhir#f",
            "https://user@mds.example/fhir",
            "",
        ] {
            assert!(Base::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn helpers() {
        assert!(valid_resource_type("Observation"));
        assert!(!valid_resource_type("observation"));
        assert!(!valid_resource_type("Observation/1"));
        assert!(!valid_resource_type(""));
        assert_eq!(
            id_from_location("https://h/fhir/Observation/42/_history/1", "Observation"),
            Some("42".into())
        );
        assert_eq!(
            id_from_location("Observation/abc-1.2", "Observation"),
            Some("abc-1.2".into())
        );
        assert_eq!(
            id_from_location("https://h/fhir/Device/42", "Observation"),
            None
        );
        assert_eq!(
            id_from_location("https://h/fhir/Observation/", "Observation"),
            None
        );
        assert_eq!(
            encode_query("2026-10-10T12:00:00.5+00:00"),
            "2026-10-10T12%3A00%3A00.5%2B00%3A00"
        );
        assert_eq!(
            outcome_text(
                &serde_json::to_vec(&json!({"resourceType":"OperationOutcome","issue":[
                    {"diagnostics":"a"},{"details":{"text":"b"}},{"severity":"error"}]}))
                .expect("json")
            ),
            Some("a; b".into())
        );
        assert_eq!(outcome_text(b"not json"), None);
        assert_eq!(outcome_text(br#"{"resourceType":"Patient"}"#), None);
    }

    #[test]
    fn bundle_parsing() {
        let mut out = Vec::new();
        let b = json!({"resourceType":"Bundle","type":"searchset",
            "link":[{"relation":"self","url":"s"},{"relation":"next","url":"n"}],
            "entry":[
                {"resource":{"resourceType":"Observation","id":"1"},"search":{"mode":"match"}},
                {"resource":{"resourceType":"OperationOutcome"},"search":{"mode":"outcome"}},
                {"resource":{"resourceType":"Observation","id":"2"}}]});
        assert_eq!(
            bundle_page(&b, "Observation", &mut out).expect("ok"),
            Some("n".into())
        );
        assert_eq!(out.len(), 2);
        let empty = json!({"resourceType":"Bundle","type":"searchset"});
        assert_eq!(
            bundle_page(&empty, "Observation", &mut out).expect("ok"),
            None
        );
        for bad in [
            json!({"resourceType":"Observation"}),
            json!({"resourceType":"Bundle","type":"history"}),
            json!({"resourceType":"Bundle","type":"searchset","entry":{}}),
            json!({"resourceType":"Bundle","type":"searchset","entry":[{}]}),
            json!({"resourceType":"Bundle","type":"searchset",
                   "entry":[{"resource":{"resourceType":"Device"}}]}),
            json!({"resourceType":"Bundle","type":"searchset",
                   "link":[{"relation":"next","url":"a"},{"relation":"next","url":"b"}]}),
        ] {
            assert!(
                matches!(
                    bundle_page(&bad, "Observation", &mut out),
                    Err(TransportError::Protocol(_))
                ),
                "{bad}"
            );
        }
    }
}
