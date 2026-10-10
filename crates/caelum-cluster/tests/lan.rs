//! Crew LAN service: authentication by client certificate and session token, role and station
//! authorisation on every endpoint, per-role projection (F-SYS-05, CLAUDE.md rule 6).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::Arc;
use std::time::Duration;

use caelum_cluster::failover::InstanceRole;
use caelum_cluster::lan::access::{authorize, AlarmPriority, Endpoint, EndpointAccess, LanDenied};
use caelum_cluster::lan::client;
use caelum_cluster::lan::server::{LanAuditEvent, StreamMessage};
use caelum_cluster::lan::tls::TlsMaterial;
use caelum_cluster::lan::{StationKind, StationRegistry};
use caelum_cluster::node::Node;
use caelum_security::role::{Command, Principal, Role};
use common::*;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;

struct Lan {
    node: Node,
    authority: Arc<TestAuthority>,
    alarms: Arc<TestAlarms>,
    tab1: TlsMaterial,
    tab2: TlsMaterial,
    kiosk: TlsMaterial,
    peer: TlsMaterial,
    rogue: TlsMaterial,
    foreign: TlsMaterial,
    pki: Pki,
}

async fn lan(timing: caelum_cluster::timing::Timing, wait_primary: bool) -> Lan {
    let pki = Pki::new("CAELUM crew LAN CA (test)");
    let (server_tls, _) = pki.station("a");
    let (tab1, fp1) = pki.station("tab-1");
    let (tab2, fp2) = pki.station("tab-2");
    let (kiosk, fpk) = pki.station("kiosk-1");
    let (peer, fpp) = pki.station("b");
    let (rogue, _) = pki.station("rogue");
    let (foreign, _) = Pki::new("Some other CA").station("tab-1");
    let mut registry = StationRegistry::new();
    registry.insert(fp1, station("tab-1", StationKind::Secondary));
    registry.insert(fp2, station("tab-2", StationKind::Secondary));
    registry.insert(fpk, station("kiosk-1", StationKind::Kiosk));
    registry.insert(fpp, station("b", StationKind::Peer));
    let authority = TestAuthority::with_crew();
    let alarms = TestAlarms::new(vec![
        alarm(1, crew(1), AlarmPriority::Critical),
        alarm(2, crew(2), AlarmPriority::Warning),
        alarm(3, crew(2), AlarmPriority::Observe),
    ]);
    let log = MemLog::default();
    let fences = MemFences::default();
    let node = Node::start(
        config("a", timing),
        parts(
            server_tls,
            "a",
            1,
            &log,
            &fences,
            registry,
            authority.clone(),
            alarms.clone(),
        ),
    )
    .await
    .unwrap();
    if wait_primary {
        assert!(
            wait_until(Duration::from_secs(5), || node.status().role
                == InstanceRole::Primary)
            .await
        );
    }
    Lan {
        node,
        authority,
        alarms,
        tab1,
        tab2,
        kiosk,
        peer,
        rogue,
        foreign,
        pki,
    }
}

async fn ws_open(
    tls: &TlsMaterial,
    lan: &Lan,
    path: &str,
    token: Option<&str>,
) -> Result<tokio_tungstenite::WebSocketStream<client::LanStream>, client::ClientError> {
    let stream = client::connect(
        tls.client_config().unwrap(),
        lan.node.local_addr(),
        "localhost",
        None,
    )
    .await?;
    client::websocket(stream, path, token).await
}

async fn next_message(
    ws: &mut tokio_tungstenite::WebSocketStream<client::LanStream>,
) -> StreamMessage {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("stream message in time")
            .expect("stream open")
            .expect("no stream error");
        if let Message::Text(text) = message {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

fn refused_with(result: Result<impl Sized, client::ClientError>, code: &str) {
    match result {
        Ok(_) => panic!("expected refusal {code}"),
        Err(e) => assert!(e.to_string().contains(code), "{e} should contain {code}"),
    }
}

#[test]
fn f_sys_05_endpoint_table_matches_the_role_matrix() {
    let kinds = [
        StationKind::Peer,
        StationKind::Secondary,
        StationKind::Kiosk,
    ];
    for endpoint in Endpoint::ALL {
        for kind in kinds {
            let st = station("s", kind);
            for role in Role::ALL {
                let u = user("u", role, Some(crew(1)), 1);
                let got = authorize(endpoint, &st, Some(&u));
                let expected = match endpoint.access() {
                    EndpointAccess::Stations(k) => k.contains(&kind),
                    EndpointAccess::User { stations, command } => {
                        stations.contains(&kind)
                            && command
                                .is_none_or(|c| c.authorize(&Principal::User(u.clone())).is_ok())
                    }
                };
                assert_eq!(got.is_ok(), expected, "{endpoint:?} {kind:?} {role:?}");
            }
            // Without a session, user endpoints are refused as unauthenticated (or by station).
            if let EndpointAccess::User { stations, .. } = endpoint.access() {
                let want = if stations.contains(&kind) {
                    LanDenied::Unauthenticated
                } else {
                    LanDenied::Station
                };
                assert_eq!(authorize(endpoint, &st, None), Err(want));
            }
        }
    }
    // The alarm endpoints follow `alarm_list` exactly; administrators and gateways are refused.
    assert_eq!(
        Endpoint::AlarmList.access(),
        EndpointAccess::User {
            stations: &[StationKind::Secondary],
            command: Some(Command::AlarmList)
        }
    );
    let sec = station("tab", StationKind::Secondary);
    for role in [Role::Administrator, Role::DeviceGateway] {
        let u = user("u", role, None, 1);
        assert_eq!(
            authorize(Endpoint::AlarmList, &sec, Some(&u)),
            Err(LanDenied::Role)
        );
        assert_eq!(
            authorize(Endpoint::AlarmStream, &sec, Some(&u)),
            Err(LanDenied::Role)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn f_sys_05_clients_without_a_provisioned_certificate_are_refused() {
    let lan = lan(fast_timing(), true).await;
    let addr = lan.node.local_addr();

    // No client certificate at all.
    let mut roots = rustls::RootCertStore::empty();
    roots.add(lan.pki.ca_der()).unwrap();
    let no_cert = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let outcome = async {
        let s = client::connect(Arc::new(no_cert), addr, "localhost", None).await?;
        client::request(s, "GET".parse().unwrap(), "/v1/status", None, None).await
    }
    .await;
    assert!(outcome.is_err(), "no client certificate: {outcome:?}");

    // Issued by another CA.
    assert!(http(&lan.foreign, addr, "GET", "/v1/status", None, None)
        .await
        .is_err());

    // Issued by the crew-LAN CA but not provisioned: dropped and audited.
    assert!(http(&lan.rogue, addr, "GET", "/v1/status", None, None)
        .await
        .is_err());
    assert!(lan
        .authority
        .events()
        .iter()
        .any(|e| matches!(e, LanAuditEvent::UnknownStation { .. })));

    // A provisioned station gets the status (no physiology in it).
    let (status, body) = http(&lan.tab1, addr, "GET", "/v1/status", None, None)
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["role"], "primary");
}

#[tokio::test(flavor = "multi_thread")]
async fn f_sys_05_user_endpoints_refuse_requests_without_a_valid_session() {
    let lan = lan(fast_timing(), true).await;
    let addr = lan.node.local_addr();
    let bogus = "0".repeat(64);
    for token in [None, Some(bogus.as_str())] {
        let (status, body) = http(&lan.tab1, addr, "GET", "/v1/alarms", token, None)
            .await
            .unwrap();
        assert_eq!(status, 401, "{body}");
        assert_eq!(body["error"], "unauthenticated");
        assert!(body.get("alarms").is_none());
        let (status, _) = http(&lan.tab1, addr, "DELETE", "/v1/sessions", token, None)
            .await
            .unwrap();
        assert_eq!(status, 401);
        refused_with(
            ws_open(&lan.tab1, &lan, "/v1/alarms/stream", token).await,
            "401",
        );
    }
    // Wrong credentials: refused and audited.
    let (status, body) = http(
        &lan.tab1,
        addr,
        "POST",
        "/v1/sessions",
        None,
        Some(serde_json::json!({ "username": "cmo", "secret": "wrong" })),
    )
    .await
    .unwrap();
    assert_eq!(status, 401);
    assert!(body.get("token").is_none());
    assert!(lan.authority.events().iter().any(|e| matches!(
        e,
        LanAuditEvent::SignInFailed { username, .. } if username == "cmo"
    )));

    // A token is bound to the station it was issued to.
    let token = sign_in(&lan.tab1, addr, "cmo", "2222").await;
    let (status, _) = http(&lan.tab2, addr, "GET", "/v1/alarms", Some(&token), None)
        .await
        .unwrap();
    assert_eq!(status, 401);
    let (status, _) = http(&lan.tab1, addr, "GET", "/v1/alarms", Some(&token), None)
        .await
        .unwrap();
    assert_eq!(status, 200);

    // Sign-out ends the session.
    let (status, _) = http(
        &lan.tab1,
        addr,
        "DELETE",
        "/v1/sessions",
        Some(&token),
        None,
    )
    .await
    .unwrap();
    assert_eq!(status, 204);
    let (status, _) = http(&lan.tab1, addr, "GET", "/v1/alarms", Some(&token), None)
        .await
        .unwrap();
    assert_eq!(status, 401);

    // A disabled account loses its session.
    let token = sign_in(&lan.tab1, addr, "bme", "5555").await;
    lan.authority.disable("bme");
    let (status, _) = http(&lan.tab1, addr, "GET", "/v1/alarms", Some(&token), None)
        .await
        .unwrap();
    assert_eq!(status, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn f_sys_05_wrong_role_and_wrong_station_are_refused() {
    let lan = lan(fast_timing(), true).await;
    let addr = lan.node.local_addr();

    for (name, secret) in [("admin", "4444"), ("gateway", "6666")] {
        let token = sign_in(&lan.tab1, addr, name, secret).await;
        let (status, body) = http(&lan.tab1, addr, "GET", "/v1/alarms", Some(&token), None)
            .await
            .unwrap();
        assert_eq!(status, 403, "{name}");
        assert_eq!(body["error"], "role");
        refused_with(
            ws_open(&lan.tab1, &lan, "/v1/alarms/stream", Some(&token)).await,
            "403",
        );
    }

    // Kiosk: no sign-in, no user endpoints, even with a valid token from elsewhere.
    let cmo = sign_in(&lan.tab1, addr, "cmo", "2222").await;
    let (status, body) = http(
        &lan.kiosk,
        addr,
        "POST",
        "/v1/sessions",
        None,
        Some(serde_json::json!({ "username": "cmo", "secret": "2222" })),
    )
    .await
    .unwrap();
    assert_eq!((status, body["error"].as_str()), (403, Some("station")));
    let (status, _) = http(&lan.kiosk, addr, "GET", "/v1/alarms", Some(&cmo), None)
        .await
        .unwrap();
    assert_eq!(status, 403);

    // Peer certificate: replication and status only.
    let (status, _) = http(
        &lan.peer,
        addr,
        "POST",
        "/v1/sessions",
        None,
        Some(serde_json::json!({ "username": "cmo", "secret": "2222" })),
    )
    .await
    .unwrap();
    assert_eq!(status, 403);
    refused_with(
        ws_open(&lan.peer, &lan, "/v1/emergency/stream", None).await,
        "403",
    );

    // Secondary and kiosk stations cannot open the replication channel.
    refused_with(
        ws_open(&lan.tab1, &lan, "/v1/cluster/replicate", Some(&cmo)).await,
        "403",
    );
    refused_with(
        ws_open(&lan.kiosk, &lan, "/v1/cluster/replicate", None).await,
        "403",
    );

    let refusals = lan
        .authority
        .events()
        .iter()
        .filter(|e| matches!(e, LanAuditEvent::Refused { .. }))
        .count();
    assert!(refusals >= 8, "every refusal audited ({refusals})");
}

#[tokio::test(flavor = "multi_thread")]
async fn f_sys_05_alarm_list_is_projected_per_role_and_scope() {
    let lan = lan(fast_timing(), true).await;
    let addr = lan.node.local_addr();
    let list = |name: &'static str, secret: &'static str| {
        let tls = &lan.tab1;
        async move {
            let token = sign_in(tls, addr, name, secret).await;
            let (status, body) = http(tls, addr, "GET", "/v1/alarms", Some(&token), None)
                .await
                .unwrap();
            assert_eq!(status, 200);
            body
        }
    };

    // Astronaut: own alarms only, with detail.
    let astro = list("astro1", "1111").await;
    let alarms = astro["alarms"].as_array().unwrap();
    assert_eq!(alarms.len(), 1);
    assert_eq!(alarms[0]["ops"]["crew_id"], crew(1).to_string());
    assert_eq!(alarms[0]["detail"]["parameter"], "heart_rate");

    // CMO: everyone, with detail.
    let cmo = list("cmo", "2222").await;
    assert_eq!(cmo["alarms"].as_array().unwrap().len(), 3);
    assert!(cmo["alarms"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a.get("detail").is_some()));

    // MCC: everyone, OPS fields only: no value, parameter name, rule id or free text.
    let mcc = list("mcc", "3333").await;
    assert_eq!(mcc["alarms"].as_array().unwrap().len(), 3);
    let text = mcc.to_string();
    for physiology in ["detail", "heart_rate", "182", "hr.high"] {
        assert!(
            !text.contains(physiology),
            "MCC view leaks {physiology}: {text}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn f_sys_05_alarm_streams_deliver_projected_snapshots_and_updates() {
    let lan = lan(fast_timing(), true).await;
    let addr = lan.node.local_addr();
    let cmo = sign_in(&lan.tab1, addr, "cmo", "2222").await;
    let mcc = sign_in(&lan.tab2, addr, "mcc", "3333").await;
    let mut cmo_ws = ws_open(&lan.tab1, &lan, "/v1/alarms/stream", Some(&cmo))
        .await
        .unwrap();
    let mut mcc_ws = ws_open(&lan.tab2, &lan, "/v1/alarms/stream", Some(&mcc))
        .await
        .unwrap();
    let mut kiosk_ws = ws_open(&lan.kiosk, &lan, "/v1/emergency/stream", None)
        .await
        .unwrap();

    let StreamMessage::Snapshot { alarms } = next_message(&mut cmo_ws).await else {
        panic!("snapshot first")
    };
    assert_eq!(alarms.len(), 3);
    let StreamMessage::Snapshot { alarms } = next_message(&mut mcc_ws).await else {
        panic!("snapshot first")
    };
    assert!(alarms.iter().all(|a| a.detail.is_none()));
    // Kiosk: crew-wide emergency view: critical and warning alarms, OPS fields only.
    let StreamMessage::Snapshot { alarms } = next_message(&mut kiosk_ws).await else {
        panic!("snapshot first")
    };
    assert_eq!(alarms.len(), 2);
    assert!(alarms.iter().all(|a| a.detail.is_none()));
    assert!(lan.authority.events().iter().any(|e| matches!(
        e,
        LanAuditEvent::EmergencyViewOpened { station } if station.as_str() == "kiosk-1"
    )));

    lan.alarms
        .publish(alarm(4, crew(1), AlarmPriority::Critical));
    let StreamMessage::Update { alarm: a } = next_message(&mut cmo_ws).await else {
        panic!("update")
    };
    assert!(a.detail.is_some());
    let StreamMessage::Update { alarm: a } = next_message(&mut mcc_ws).await else {
        panic!("update")
    };
    assert!(a.detail.is_none());
    let StreamMessage::Update { alarm: a } = next_message(&mut kiosk_ws).await else {
        panic!("update")
    };
    assert!(a.detail.is_none());

    // Signing out closes the user's stream within the re-check period.
    http(&lan.tab1, addr, "DELETE", "/v1/sessions", Some(&cmo), None)
        .await
        .unwrap();
    let StreamMessage::Closed { reason } = next_message(&mut cmo_ws).await else {
        panic!("closed")
    };
    assert_eq!(reason, "session_ended");
}

#[tokio::test(flavor = "multi_thread")]
async fn f_sys_05_a_standby_serves_status_but_no_crew_data() {
    // Default timing: the node listens 5 s before it may promote, so it is a standby now.
    let lan = lan(caelum_cluster::timing::Timing::default(), false).await;
    let addr = lan.node.local_addr();
    let (status, body) = http(&lan.tab1, addr, "GET", "/v1/status", None, None)
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["role"], "standby");
    let (status, body) = http(
        &lan.tab1,
        addr,
        "POST",
        "/v1/sessions",
        None,
        Some(serde_json::json!({ "username": "cmo", "secret": "2222" })),
    )
    .await
    .unwrap();
    assert_eq!((status, body["error"].as_str()), (503, Some("not_primary")));
    refused_with(
        ws_open(&lan.kiosk, &lan, "/v1/emergency/stream", None).await,
        "503",
    );
}
