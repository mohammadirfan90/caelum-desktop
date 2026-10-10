//! HTTPS transport against a loopback TLS 1.3 server with mutual authentication (F-GND-12,
//! R-INT-05, ICD-MDS §3–4, §6, §9; CR-016 "HTTPS transport"). Certificates are generated per
//! test with `rcgen`; nothing leaves the loopback interface.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use caelum_mds::https::{HttpsConfig, HttpsConfigError, HttpsTransport, SEARCH_PAGE_BUDGET};
use caelum_mds::ids::{ClientId, ResourceKind};
use caelum_mds::transport::{FhirTransport, SearchQuery, TransportError};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{
    RootCertStore, ServerConfig, ServerConnection, StreamOwned, SupportedProtocolVersion,
};
use serde_json::{json, Value};
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------------------------
// Test PKI

struct Ca {
    issuer: Issuer<'static, KeyPair>,
    pem: String,
}

struct Leaf {
    cert_pem: String,
    key_pem: String,
}

fn ca(name: &str) -> Ca {
    let key = KeyPair::generate().expect("key");
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("params");
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let cert = params.self_signed(&key).expect("self-signed");
    Ca {
        pem: cert.pem(),
        issuer: Issuer::new(params, key),
    }
}

fn leaf(ca: &Ca, sans: &[&str], usage: ExtendedKeyUsagePurpose) -> Leaf {
    let key = KeyPair::generate().expect("key");
    let mut params = CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .expect("params");
    params
        .distinguished_name
        .push(DnType::CommonName, "caelum test");
    params.extended_key_usages = vec![usage];
    let cert = params.signed_by(&key, &ca.issuer).expect("signed");
    Leaf {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    }
}

/// The certificates of one test: the mission CA, the MDS server and this CAELUM instance.
struct Pki {
    ca: Ca,
    server: Leaf,
    client: Leaf,
}

fn pki() -> Pki {
    let ca = ca("mission CA");
    let server = leaf(&ca, &["127.0.0.1"], ExtendedKeyUsagePurpose::ServerAuth);
    let client = leaf(
        &ca,
        &["caelum-1.crew.local"],
        ExtendedKeyUsagePurpose::ClientAuth,
    );
    Pki { ca, server, client }
}

// ---------------------------------------------------------------------------------------------
// Loopback TLS server

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn server_config(
    server: &Leaf,
    client_ca_pem: &str,
    versions: &[&'static SupportedProtocolVersion],
) -> Arc<ServerConfig> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(client_ca_pem.as_bytes()).expect("ca"))
        .expect("root");
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider())
        .build()
        .expect("verifier");
    let cert = CertificateDer::from_pem_slice(server.cert_pem.as_bytes()).expect("cert");
    let key = PrivateKeyDer::from_pem_slice(server.key_pem.as_bytes()).expect("key");
    Arc::new(
        ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(versions)
            .expect("versions")
            .with_client_cert_verifier(verifier)
            .with_single_cert(vec![cert], key)
            .expect("server config"),
    )
}

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

enum Reply {
    Respond {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    },
    /// Hold the connection without answering.
    Stall(Duration),
}

fn respond(status: u16, body: Value) -> Reply {
    Reply::Respond {
        status,
        headers: vec![("Content-Type", "application/fhir+json".into())],
        body: serde_json::to_vec(&body).expect("json"),
    }
}

fn raw(status: u16, headers: Vec<(&'static str, String)>, body: &[u8]) -> Reply {
    Reply::Respond {
        status,
        headers,
        body: body.to_vec(),
    }
}

type Handler = dyn Fn(u16, &Recorded) -> Reply + Send + Sync;

struct Server {
    port: u16,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Server {
    fn base(&self) -> String {
        format!("https://127.0.0.1:{}/fhir", self.port)
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("lock").clone()
    }
}

fn serve(
    config: Arc<ServerConfig>,
    handler: impl Fn(u16, &Recorded) -> Reply + Send + Sync + 'static,
) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler: Arc<Handler> = Arc::new(handler);
    let reqs = requests.clone();
    thread::spawn(move || {
        for tcp in listener.incoming() {
            let Ok(tcp) = tcp else { continue };
            let (config, handler, reqs) = (config.clone(), handler.clone(), reqs.clone());
            // A refused handshake is an expected outcome in several tests; the assertions
            // look at which requests reached the handler.
            thread::spawn(move || {
                let _ = connection(config, tcp, port, &*handler, &reqs);
            });
        }
    });
    Server { port, requests }
}

fn connection(
    config: Arc<ServerConfig>,
    tcp: TcpStream,
    port: u16,
    handler: &Handler,
    requests: &Mutex<Vec<Recorded>>,
) -> Result<(), String> {
    tcp.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let conn = ServerConnection::new(config).map_err(|e| e.to_string())?;
    let mut tls = StreamOwned::new(conn, tcp);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = tls.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed before request".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap_or("").split(' ');
    let method = start.next().unwrap_or("").to_owned();
    let target = start.next().unwrap_or("").to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.parse().expect("length"))
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = tls.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed in body".into());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let req = Recorded {
        method,
        target,
        headers,
        body,
    };
    requests.lock().expect("lock").push(req.clone());
    match handler(port, &req) {
        Reply::Stall(d) => {
            thread::sleep(d);
            Ok(())
        }
        Reply::Respond {
            status,
            headers,
            body,
        } => {
            let mut out = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
            for (k, v) in headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
            let mut bytes = out.into_bytes();
            bytes.extend_from_slice(&body);
            tls.write_all(&bytes).map_err(|e| e.to_string())?;
            tls.conn.send_close_notify();
            tls.flush().map_err(|e| e.to_string())
        }
    }
}

fn tls13_server(
    p: &Pki,
    handler: impl Fn(u16, &Recorded) -> Reply + Send + Sync + 'static,
) -> Server {
    serve(
        server_config(&p.server, &p.ca.pem, &[&rustls::version::TLS13]),
        handler,
    )
}

// ---------------------------------------------------------------------------------------------
// Client helpers

fn config(base: &str, ca_pem: &str, client: &Leaf) -> HttpsConfig {
    HttpsConfig {
        base_url: base.to_owned(),
        ca_pem: ca_pem.as_bytes().to_vec(),
        client_cert_pem: client.cert_pem.as_bytes().to_vec(),
        client_key_pem: Zeroizing::new(client.key_pem.as_bytes().to_vec()),
        timeout: Duration::from_secs(5),
    }
}

fn transport(server: &Server, p: &Pki) -> HttpsTransport {
    HttpsTransport::new(config(&server.base(), &p.ca.pem, &p.client)).expect("transport")
}

fn client_id() -> ClientId {
    ClientId::derive(ResourceKind::Observation, "hr-1")
}

fn obs() -> Value {
    json!({ "resourceType": "Observation", "status": "final",
            "identifier": [{ "system": "urn:caelum:client-id", "value": client_id().to_string() }] })
}

fn create(t: &mut HttpsTransport) -> Result<caelum_mds::transport::CreateOutcome, TransportError> {
    t.create_conditional("Observation", client_id(), &obs())
}

fn query(ge: Option<&str>, count: u32) -> SearchQuery {
    SearchQuery {
        resource_type: "Observation".into(),
        last_updated_ge: ge.map(str::to_owned),
        count,
    }
}

fn bundle(ids: &[&str], next: Option<String>) -> Value {
    let mut b = json!({
        "resourceType": "Bundle", "type": "searchset",
        "entry": ids.iter().map(|id| json!({
            "resource": { "resourceType": "Observation", "id": id },
            "search": { "mode": "match" } })).collect::<Vec<_>>(),
    });
    if let Some(n) = next {
        b["link"] = json!([{ "relation": "next", "url": n }]);
    }
    b
}

fn ids(resources: &[Value]) -> Vec<&str> {
    resources.iter().filter_map(|r| r["id"].as_str()).collect()
}

// ---------------------------------------------------------------------------------------------
// TLS: mutual authentication, trust anchors, server name, version

/// F-GND-12, R-INT-05, ICD-MDS §3: the server requires a client certificate; the transport
/// presents its own and is accepted; a client without one, or with one from another CA, is
/// refused before any request is processed.
#[test]
fn f_gnd_12_r_int_05_mutual_tls_required() {
    let p = pki();
    let server = tls13_server(&p, |_, _| {
        respond(201, json!({"resourceType":"Observation","id":"1"}))
    });

    // A plain TLS 1.3 client that trusts the CA but has no certificate.
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(p.ca.pem.as_bytes()).expect("ca"))
        .expect("root");
    let anon = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    let conn = rustls::ClientConnection::new(Arc::new(anon), "127.0.0.1".try_into().expect("name"))
        .expect("conn");
    let tcp = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    let mut tls = StreamOwned::new(conn, tcp);
    let _ = tls.write_all(b"GET /fhir/Observation HTTP/1.1\r\nHost: x\r\n\r\n");
    let mut sink = Vec::new();
    assert!(
        tls.read_to_end(&mut sink).is_err() || sink.is_empty(),
        "the server must refuse a client without a certificate"
    );
    assert!(
        server.requests().is_empty(),
        "no request reached the handler"
    );

    // A client certificate from a different CA.
    let rogue_ca = ca("rogue CA");
    let rogue = leaf(
        &rogue_ca,
        &["caelum-1.crew.local"],
        ExtendedKeyUsagePurpose::ClientAuth,
    );
    let mut t = HttpsTransport::new(config(&server.base(), &p.ca.pem, &rogue)).expect("transport");
    assert!(matches!(
        create(&mut t),
        Err(TransportError::Unreachable(_))
    ));
    assert!(
        server.requests().is_empty(),
        "no request reached the handler"
    );

    // The proper client certificate is accepted.
    let mut t = transport(&server, &p);
    assert_eq!(create(&mut t).expect("created").server_id, "1");
    assert_eq!(server.requests().len(), 1);
}

/// R-INT-05, ICD-MDS §3: only the mission CA is trusted (never the web roots); a server whose
/// certificate chains to another CA, or does not name the host, is refused.
#[test]
fn r_int_05_wrong_ca_and_wrong_name_refused() {
    let p = pki();
    let server = tls13_server(&p, |_, _| {
        respond(201, json!({"resourceType":"Observation","id":"1"}))
    });
    let other = ca("other CA");
    let mut t =
        HttpsTransport::new(config(&server.base(), &other.pem, &p.client)).expect("transport");
    assert!(matches!(
        create(&mut t),
        Err(TransportError::Unreachable(_))
    ));

    let misnamed = leaf(
        &p.ca,
        &["mds.other.local"],
        ExtendedKeyUsagePurpose::ServerAuth,
    );
    let server2 = serve(
        server_config(&misnamed, &p.ca.pem, &[&rustls::version::TLS13]),
        |_, _| respond(201, json!({"resourceType":"Observation","id":"1"})),
    );
    let mut t = transport(&server2, &p);
    assert!(matches!(
        create(&mut t),
        Err(TransportError::Unreachable(_))
    ));
    assert!(server.requests().is_empty() && server2.requests().is_empty());
}

/// ICD-MDS §3: TLS 1.3 only; a server limited to TLS 1.2 is refused.
#[test]
fn r_int_05_tls12_server_refused() {
    let p = pki();
    let server = serve(
        server_config(&p.server, &p.ca.pem, &[&rustls::version::TLS12]),
        |_, _| respond(201, json!({"resourceType":"Observation","id":"1"})),
    );
    let mut t = transport(&server, &p);
    assert!(matches!(
        create(&mut t),
        Err(TransportError::Unreachable(_))
    ));
    assert!(server.requests().is_empty());
}

/// ICD-MDS §9: nothing listening is a link failure (retried).
#[test]
fn r_int_05_connection_refused_is_unreachable() {
    let p = pki();
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr").port()
    };
    let mut t = HttpsTransport::new(config(
        &format!("https://127.0.0.1:{port}/fhir"),
        &p.ca.pem,
        &p.client,
    ))
    .expect("transport");
    let e = create(&mut t).expect_err("refused");
    assert!(matches!(e, TransportError::Unreachable(_)), "{e:?}");
    assert!(e.is_retryable());
    assert!(t.search(&query(None, 10)).is_err());
}

// ---------------------------------------------------------------------------------------------
// Conditional create

/// F-GND-12, ICD-MDS §4: `POST [base]/Observation` with the conditional-create header and FHIR
/// JSON; 201 is a new resource, its id taken from the returned resource.
#[test]
fn f_gnd_12_conditional_create_201_sends_if_none_exist() {
    let p = pki();
    let server = tls13_server(&p, |_, _| {
        respond(
            201,
            json!({"resourceType":"Observation","id":"obs-7","meta":{"versionId":"1"}}),
        )
    });
    let mut t = transport(&server, &p);
    let out = create(&mut t).expect("created");
    assert_eq!((out.server_id.as_str(), out.created), ("obs-7", true));
    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(
        (r.method.as_str(), r.target.as_str()),
        ("POST", "/fhir/Observation")
    );
    assert_eq!(
        r.header("If-None-Exist"),
        Some(format!("identifier=urn:caelum:client-id|{}", client_id()).as_str())
    );
    assert_eq!(r.header("Content-Type"), Some("application/fhir+json"));
    assert_eq!(r.header("Accept"), Some("application/fhir+json"));
    let sent: Value = serde_json::from_slice(&r.body).expect("json body");
    assert_eq!(sent, obs());
}

/// F-GND-12, ICD-MDS §4: 200 means the resource already existed (a retry after a lost
/// response); the id comes from `Location` when the body has none.
#[test]
fn f_gnd_12_conditional_create_200_existing_from_location() {
    let p = pki();
    let server = tls13_server(&p, |port, _| {
        raw(
            200,
            vec![(
                "Location",
                format!("https://127.0.0.1:{port}/fhir/Observation/77/_history/2"),
            )],
            b"",
        )
    });
    let mut t = transport(&server, &p);
    let out = create(&mut t).expect("existing");
    assert_eq!((out.server_id.as_str(), out.created), ("77", false));
}

/// ICD-MDS §9: a create answer without a usable id, or with an unexpected status, is a protocol
/// error (retried; the conditional create keeps the retry idempotent).
#[test]
fn r_int_05_create_without_id_or_unexpected_status_is_protocol() {
    let p = pki();
    for body in [json!({}), json!({"resourceType":"OperationOutcome"})] {
        let server = tls13_server(&p, move |_, _| respond(201, body.clone()));
        let mut t = transport(&server, &p);
        assert!(matches!(create(&mut t), Err(TransportError::Protocol(_))));
    }
    let server = tls13_server(&p, |_, _| raw(204, vec![], b""));
    let mut t = transport(&server, &p);
    assert!(matches!(create(&mut t), Err(TransportError::Protocol(_))));
}

/// ICD-MDS §9: 4xx carries the `OperationOutcome` diagnostics and is held for review; 408 and
/// 429 are 4xx but retried; 5xx is retried.
#[test]
fn r_int_05_status_classification() {
    let p = pki();
    let server = tls13_server(&p, |_, _| {
        respond(
            422,
            json!({"resourceType":"OperationOutcome","issue":[{"severity":"error",
                "code":"processing","diagnostics":"Observation.code: minimum required = 1"}]}),
        )
    });
    let mut t = transport(&server, &p);
    let e = create(&mut t).expect_err("422");
    assert_eq!(
        e,
        TransportError::Http4xx {
            status: 422,
            outcome: "Observation.code: minimum required = 1".into()
        }
    );
    assert!(!e.is_retryable() && e.proves_contact());

    // No `OperationOutcome` body: the reason phrase is the outcome text.
    for (status, want_retry) in [(400, false), (404, false), (408, true), (429, true)] {
        let server = tls13_server(&p, move |_, _| raw(status, vec![], b""));
        let mut t = transport(&server, &p);
        let e = create(&mut t).expect_err("4xx");
        match &e {
            TransportError::Http4xx { status: s, outcome } => {
                assert_eq!(*s, status);
                assert!(
                    !outcome.is_empty(),
                    "reason phrase when no OperationOutcome"
                );
            }
            other => panic!("{status}: {other:?}"),
        }
        assert_eq!(e.is_retryable(), want_retry, "{status}");
    }
    for status in [500, 502, 503] {
        let server = tls13_server(&p, move |_, _| raw(status, vec![], b"down"));
        let mut t = transport(&server, &p);
        assert_eq!(create(&mut t), Err(TransportError::Http5xx(status)));
        assert!(t.search(&query(None, 5)) == Err(TransportError::Http5xx(status)));
    }
}

/// ICD-MDS §9: no complete response within the configured timeout is `Timeout` (retried).
#[test]
fn r_int_05_timeout() {
    let p = pki();
    let server = tls13_server(&p, |_, _| Reply::Stall(Duration::from_secs(3)));
    let mut cfg = config(&server.base(), &p.ca.pem, &p.client);
    cfg.timeout = Duration::from_millis(300);
    let mut t = HttpsTransport::new(cfg).expect("transport");
    let started = Instant::now();
    assert_eq!(create(&mut t), Err(TransportError::Timeout));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(t.search(&query(None, 5)), Err(TransportError::Timeout));
}

/// Redirects are not followed (the client certificate is never presented elsewhere).
#[test]
fn r_int_05_redirect_not_followed() {
    let p = pki();
    let server = tls13_server(&p, |port, _| {
        raw(
            307,
            vec![("Location", format!("https://127.0.0.1:{port}/elsewhere"))],
            b"",
        )
    });
    let mut t = transport(&server, &p);
    assert!(matches!(create(&mut t), Err(TransportError::Protocol(_))));
    assert!(matches!(
        t.search(&query(None, 5)),
        Err(TransportError::Protocol(_))
    ));
    assert!(server
        .requests()
        .iter()
        .all(|r| r.target.starts_with("/fhir/")));
    assert_eq!(server.requests().len(), 2);
}

/// A resource whose type does not match the endpoint is refused locally as a 4xx (held for
/// review), and nothing is sent.
#[test]
fn f_gnd_12_mismatched_resource_type_not_sent() {
    let p = pki();
    let server = tls13_server(&p, |_, _| {
        respond(201, json!({"resourceType":"Device","id":"1"}))
    });
    let mut t = transport(&server, &p);
    let e = t
        .create_conditional("Device", client_id(), &obs())
        .expect_err("mismatch");
    assert!(matches!(e, TransportError::Http4xx { status: 400, .. }) && !e.is_retryable());
    let e = t
        .create_conditional("Observation/../Device", client_id(), &obs())
        .expect_err("path");
    assert!(matches!(e, TransportError::Http4xx { status: 400, .. }));
    assert!(t.search(&query(None, 0)).is_err());
    assert!(server.requests().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Search

/// F-GND-12, ICD-MDS §6: read-back search on `_lastUpdated` with sort and page size; the bundle
/// entries are returned in order; `include` / `outcome` entries are skipped.
#[test]
fn f_gnd_12_search_query_parameters() {
    let p = pki();
    let server = tls13_server(&p, |_, _| {
        let mut b = bundle(&["a", "b"], None);
        b["entry"].as_array_mut().expect("entries").push(
            json!({"resource":{"resourceType":"OperationOutcome"},"search":{"mode":"outcome"}}),
        );
        respond(200, b)
    });
    let mut t = transport(&server, &p);
    let got = t
        .search(&query(Some("2026-10-10T12:00:00.250Z"), 50))
        .expect("search");
    assert_eq!(ids(&got), ["a", "b"]);
    let got = t.search(&query(None, 7)).expect("search");
    assert_eq!(got.len(), 2);
    let reqs = server.requests();
    assert_eq!(reqs[0].method, "GET");
    assert_eq!(
        reqs[0].target,
        "/fhir/Observation?_lastUpdated=ge2026-10-10T12%3A00%3A00.250Z&_sort=_lastUpdated&_count=50"
    );
    assert_eq!(
        reqs[1].target,
        "/fhir/Observation?_sort=_lastUpdated&_count=7"
    );
    assert_eq!(reqs[0].header("Accept"), Some("application/fhir+json"));
    assert!(reqs[0].header("If-None-Exist").is_none());
}

/// ICD-MDS §9: a search answer that is not FHIR JSON, or not a searchset bundle, is a protocol
/// error.
#[test]
fn r_int_05_bad_json_and_bad_bundle_are_protocol() {
    let p = pki();
    for body in [
        b"{not json".to_vec(),
        serde_json::to_vec(&json!({"resourceType":"Patient"})).expect("json"),
        serde_json::to_vec(&json!({"resourceType":"Bundle","type":"searchset",
            "entry":[{"resource":{"resourceType":"Device","id":"1"}}]}))
        .expect("json"),
    ] {
        let server = tls13_server(&p, move |_, _| raw(200, vec![], &body));
        let mut t = transport(&server, &p);
        let e = t.search(&query(None, 5)).expect_err("protocol");
        assert!(matches!(e, TransportError::Protocol(_)), "{e:?}");
        assert!(e.is_retryable() && !e.proves_contact());
    }
    let server = tls13_server(&p, |_, _| raw(201, vec![], b"{not json"));
    let mut t = transport(&server, &p);
    assert!(matches!(create(&mut t), Err(TransportError::Protocol(_))));
}

/// F-GND-12: `next` links are followed in order and the pages concatenated.
#[test]
fn f_gnd_12_search_follows_next_links() {
    let p = pki();
    let server = tls13_server(&p, |port, r| {
        if r.target.contains("_getpages") {
            respond(200, bundle(&["c"], None))
        } else {
            respond(
                200,
                bundle(
                    &["a", "b"],
                    Some(format!(
                        "https://127.0.0.1:{port}/fhir?_getpages=abc&_getpagesoffset=2&_count=2"
                    )),
                ),
            )
        }
    });
    let mut t = transport(&server, &p);
    let got = t.search(&query(None, 2)).expect("search");
    assert_eq!(ids(&got), ["a", "b", "c"]);
    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(
        reqs[1].target,
        "/fhir?_getpages=abc&_getpagesoffset=2&_count=2"
    );
}

/// F-GND-12: at most `SEARCH_PAGE_BUDGET` pages per search; the connector polls again for the
/// rest.
#[test]
fn f_gnd_12_search_page_budget() {
    let p = pki();
    let server = tls13_server(&p, |port, r| {
        let n = r.target.len();
        respond(
            200,
            bundle(
                &[&format!("p{n}")],
                Some(format!(
                    "https://127.0.0.1:{port}/fhir/Observation?page={n}x"
                )),
            ),
        )
    });
    let mut t = transport(&server, &p);
    let got = t.search(&query(None, 1)).expect("search");
    assert_eq!(got.len(), SEARCH_PAGE_BUDGET);
    assert_eq!(server.requests().len(), SEARCH_PAGE_BUDGET);
}

/// A `next` link outside the configured base URL is not followed.
#[test]
fn r_int_05_foreign_next_link_refused() {
    let p = pki();
    let server = tls13_server(&p, |port, _| {
        respond(
            200,
            bundle(
                &["a"],
                Some(format!("https://127.0.0.2:{port}/fhir?page=2")),
            ),
        )
    });
    let mut t = transport(&server, &p);
    assert!(matches!(
        t.search(&query(None, 1)),
        Err(TransportError::Protocol(_))
    ));
    assert_eq!(server.requests().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Configuration

/// The transport is never built from an unusable configuration (no default trust, no
/// unauthenticated mode); the key is not printed.
#[test]
fn r_int_05_configuration_validated() {
    let p = pki();
    let base = "https://127.0.0.1:1/fhir";
    let ok = config(base, &p.ca.pem, &p.client);
    assert!(HttpsTransport::new(ok.clone()).is_ok());
    assert!(!format!("{ok:?}").contains("PRIVATE KEY"));

    let mut c = ok.clone();
    c.base_url = "http://127.0.0.1:1/fhir".into();
    assert!(matches!(
        HttpsTransport::new(c),
        Err(HttpsConfigError::BaseUrl(_))
    ));
    let mut c = ok.clone();
    c.ca_pem = Vec::new();
    assert!(matches!(
        HttpsTransport::new(c),
        Err(HttpsConfigError::CaBundle(_))
    ));
    let mut c = ok.clone();
    c.ca_pem = b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_vec();
    assert!(matches!(
        HttpsTransport::new(c),
        Err(HttpsConfigError::CaBundle(_))
    ));
    let mut c = ok.clone();
    c.client_cert_pem = b"garbage".to_vec();
    assert!(matches!(
        HttpsTransport::new(c),
        Err(HttpsConfigError::ClientCertificate(_))
    ));
    let mut c = ok.clone();
    c.client_key_pem = Zeroizing::new(Vec::new());
    assert!(matches!(
        HttpsTransport::new(c),
        Err(HttpsConfigError::ClientKey(_))
    ));
    let mut c = ok.clone();
    c.client_key_pem = Zeroizing::new(p.server.key_pem.as_bytes().to_vec());
    assert!(
        matches!(HttpsTransport::new(c), Err(HttpsConfigError::ClientKey(_))),
        "key of another certificate"
    );
    let mut c = ok;
    c.timeout = Duration::ZERO;
    assert_eq!(
        HttpsTransport::new(c).err(),
        Some(HttpsConfigError::Timeout)
    );
}
