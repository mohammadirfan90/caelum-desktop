//! Two in-process nodes over loopback mutual TLS (test certificates generated here): the primary
//! is killed under load, and a partitioned stale primary is fenced on heal (F-SYS-04, F-SYS-05,
//! R-OPS-02; M11 gate).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_arguments)] // test code

mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use caelum_cluster::failover::InstanceRole;
use caelum_cluster::fence::Fence;
use caelum_cluster::lan::tls::TlsMaterial;
use caelum_cluster::lan::{Fingerprint, StationKind, StationRegistry};
use caelum_cluster::log::{verify_chain, ChainHead};
use caelum_cluster::node::{AppendError, Node, NodeEvent, PeerConfig};
use caelum_cluster::timing::{Timing, TAKEOVER_LIMIT};
use common::*;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use tokio::time::Instant;

struct Pair {
    a_tls: Option<TlsMaterial>,
    b_tls: Option<TlsMaterial>,
    a_fp: Fingerprint,
    b_fp: Fingerprint,
    a_log: MemLog,
    b_log: MemLog,
    a_fences: MemFences,
    b_fences: MemFences,
}

fn pair() -> Pair {
    let pki = Pki::new("CAELUM crew LAN CA (test)");
    let (a_tls, a_fp) = pki.station("a");
    let (b_tls, b_fp) = pki.station("b");
    Pair {
        a_tls: Some(a_tls),
        b_tls: Some(b_tls),
        a_fp,
        b_fp,
        a_log: MemLog::default(),
        b_log: MemLog::default(),
        a_fences: MemFences::default(),
        b_fences: MemFences::default(),
    }
}

fn start(
    id: &str,
    seed: u8,
    tls: TlsMaterial,
    listener: TcpListener,
    log: &MemLog,
    fences: &MemFences,
    timing: Timing,
    peer: (&str, SocketAddr, Fingerprint),
) -> Node {
    let mut registry = StationRegistry::new();
    registry.insert(peer.2, station(peer.0, StationKind::Peer));
    let mut cfg = config(id, timing);
    cfg.peer = Some(PeerConfig {
        node: node(peer.0),
        addr: peer.1,
        server_name: "localhost".into(),
        fingerprint: peer.2,
    });
    Node::start_on(
        listener,
        cfg,
        parts(
            tls,
            id,
            seed,
            log,
            fences,
            registry,
            TestAuthority::with_crew(),
            TestAlarms::new(Vec::new()),
        ),
    )
    .unwrap()
}

async fn listener() -> (TcpListener, SocketAddr) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    (l, addr)
}

fn drain(rx: &mut broadcast::Receiver<NodeEvent>) -> Vec<NodeEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f_sys_04_r_ops_02_primary_killed_under_load_standby_takes_over_within_10_s() {
    let mut p = pair();
    let (la, a_addr) = listener().await;
    let (lb, b_addr) = listener().await;
    // Default timing: 1 s heartbeat, 5 missed, 4 s device reconnection budget.
    let timing = Timing::default();
    let a = start(
        "a",
        1,
        p.a_tls.take().unwrap(),
        la,
        &p.a_log,
        &p.a_fences,
        timing,
        ("b", b_addr, p.b_fp),
    );
    assert!(
        wait_until(Duration::from_secs(7), || a.status().role
            == InstanceRole::Primary)
        .await
    );
    let b = start(
        "b",
        2,
        p.b_tls.take().unwrap(),
        lb,
        &p.b_log,
        &p.b_fences,
        timing,
        ("a", a_addr, p.a_fp),
    );
    let mut b_events = b.events();
    assert!(
        wait_until(Duration::from_secs(5), || {
            b.status().primary == Some(Fence::new(1, node("a"))) && a.standby_acked().is_some()
        })
        .await,
        "standby follows the primary"
    );
    assert_eq!(b.status().role, InstanceRole::Standby);
    assert_eq!(
        b.append(body(0)),
        Err(AppendError::NotPrimary),
        "standby is read-only"
    );

    // Full load: append continuously for 3 s.
    let load_until = Instant::now() + Duration::from_secs(3);
    let mut appended = 0u64;
    while Instant::now() < load_until {
        for _ in 0..50 {
            appended = a.append(body(appended)).unwrap();
        }
        tokio::task::yield_now().await;
    }
    let acked = a.standby_acked().expect("standby connected");
    // Kill the primary: every task and connection stops at once.
    drop(a);
    let killed_at = Instant::now();
    assert!(appended > 1_000, "load: {appended} entries");
    assert!(acked > 0, "acknowledged under load: {acked}");

    assert!(
        wait_until(TAKEOVER_LIMIT, || b.status().role == InstanceRole::Primary).await,
        "takeover within 10 s"
    );
    let takeover = killed_at.elapsed();
    assert!(takeover <= TAKEOVER_LIMIT, "takeover {takeover:?}");
    let events = drain(&mut b_events);
    let deadline = events
        .iter()
        .find_map(|e| match e {
            NodeEvent::ReconnectDevices { fence, deadline } => {
                assert_eq!(fence, &Fence::new(2, node("b")));
                Some(*deadline)
            }
            _ => None,
        })
        .expect("device hand-off signalled");
    assert!(
        deadline.duration_since(killed_at) <= TAKEOVER_LIMIT,
        "promotion plus device reconnection budget within 10 s: {:?}",
        deadline.duration_since(killed_at)
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, NodeEvent::BecamePrimary { .. })));

    // No acknowledged entry lost: everything up to `acked` is on the new primary, identical.
    let a_entries = p.a_log.entries();
    let b_entries = p.b_log.entries();
    assert!(b_entries.len() as u64 >= acked);
    for seq in 1..=acked as usize {
        assert_eq!(b_entries[seq - 1], a_entries[seq - 1], "seq {seq}");
    }
    verify_chain(&ChainHead::genesis(), &b_entries, &trust_ab()).unwrap();
    assert_eq!(
        p.b_fences.get(),
        Some(Fence::new(2, node("b"))),
        "fence persisted"
    );

    // The new primary continues the chain under its own fence.
    let seq = b.append(body(1_000_000)).unwrap();
    assert_eq!(seq, b_entries.len() as u64 + 1);
    let last = p.b_log.entries().pop().unwrap();
    assert_eq!(last.fence, Fence::new(2, node("b")));
}

/// A TCP relay that can be cut (partition) and healed.
struct Relay {
    addr: SocketAddr,
    cut: Arc<AtomicBool>,
    connections: Arc<Mutex<Vec<AbortHandle>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Relay {
    async fn start(target: SocketAddr) -> Relay {
        let (l, addr) = listener().await;
        let cut = Arc::new(AtomicBool::new(false));
        let connections: Arc<Mutex<Vec<AbortHandle>>> = Arc::default();
        let (c, conns) = (cut.clone(), connections.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut inbound, _)) = l.accept().await else {
                    continue;
                };
                if c.load(Ordering::SeqCst) {
                    continue;
                }
                let handle = tokio::spawn(async move {
                    if let Ok(mut outbound) = TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
                conns.lock().unwrap().push(handle.abort_handle());
            }
        });
        Relay {
            addr,
            cut,
            connections,
            task,
        }
    }

    fn set_cut(&self, cut: bool) {
        self.cut.store(cut, Ordering::SeqCst);
        if cut {
            for c in self.connections.lock().unwrap().drain(..) {
                c.abort();
            }
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
        self.set_cut(true);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f_sys_04_partitioned_stale_primary_is_fenced_and_its_unacknowledged_entries_quarantined() {
    let mut p = pair();
    let (la, a_addr) = listener().await;
    let (lb, b_addr) = listener().await;
    let to_b = Relay::start(b_addr).await;
    let to_a = Relay::start(a_addr).await;
    let timing = fast_timing();
    let a = start(
        "a",
        1,
        p.a_tls.take().unwrap(),
        la,
        &p.a_log,
        &p.a_fences,
        timing,
        ("b", to_b.addr, p.b_fp),
    );
    let mut a_events = a.events();
    assert!(
        wait_until(Duration::from_secs(3), || a.status().role
            == InstanceRole::Primary)
        .await
    );
    let b = start(
        "b",
        2,
        p.b_tls.take().unwrap(),
        lb,
        &p.b_log,
        &p.b_fences,
        timing,
        ("a", to_a.addr, p.a_fp),
    );
    for i in 0..100 {
        a.append(body(i)).unwrap();
    }
    assert!(a.wait_acknowledged(100, Duration::from_secs(5)).await);

    // Partition: b takes over; a cannot know and keeps acting locally (alarms stay local).
    to_b.set_cut(true);
    to_a.set_cut(true);
    assert!(
        wait_until(Duration::from_secs(3), || b.status().role
            == InstanceRole::Primary)
        .await
    );
    assert_eq!(a.status().role, InstanceRole::Primary);
    for i in 100..120 {
        a.append(body(i)).unwrap();
    }
    for i in 0..10 {
        b.append(body(10_000 + i)).unwrap();
    }
    assert!(
        !a.wait_acknowledged(101, Duration::from_millis(500)).await,
        "a stale primary never gets an entry acknowledged"
    );

    // Heal: a is fenced, demotes, quarantines its unacknowledged entries and follows b.
    to_b.set_cut(false);
    to_a.set_cut(false);
    assert!(
        wait_until(Duration::from_secs(5), || {
            a.status().role == InstanceRole::Standby && p.a_log.entries() == p.b_log.entries()
        })
        .await,
        "stale primary demoted and converged"
    );
    assert_eq!(a.status().primary, Some(Fence::new(2, node("b"))));
    assert_eq!(a.append(body(999)), Err(AppendError::NotPrimary));
    let events = drain(&mut a_events);
    assert!(events.contains(&NodeEvent::Demoted {
        from: Fence::new(1, node("a")),
        to: Fence::new(2, node("b")),
    }));
    let quarantined = p.a_log.quarantine();
    assert_eq!(
        quarantined.len(),
        20,
        "unacknowledged entries kept for review"
    );
    assert!(quarantined
        .iter()
        .all(|e| e.fence == Fence::new(1, node("a")) && e.seq > 100));
    assert!(events
        .iter()
        .any(|e| matches!(e, NodeEvent::Diverted { entries } if entries.len() == 20)));
    assert!(b.wait_acknowledged(110, Duration::from_secs(5)).await);
    assert_eq!(b.status().role, InstanceRole::Primary);
}
