//! Uplink through the station (F-GND-07, R-SEC-05, SR-009; CR-016): an uplinked item is applied
//! only with a valid signature from a trusted key, by an administrator after re-authentication,
//! and audited; a ruleset takes effect at the next start.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::security::role::{DeniedReason, Role};
use caelum_core::station::{Station, StationError};
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";

fn station() -> (DataDir, Station) {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    clock
        .0
        .tai
        .store(2_100_000_000_000_000_000, Ordering::SeqCst);
    let mut s = open(&dir, &keys, &clock).unwrap();
    s.create_first_administrator("admin", ADMIN).unwrap();
    s.login("admin", ADMIN.as_bytes()).unwrap();
    s.reauth(ADMIN.as_bytes()).unwrap();
    (dir, s)
}

fn fixture(dir: &str, name: &str) -> (Vec<u8>, String) {
    let d = format!("{}/tests/fixtures/{dir}/", env!("CARGO_MANIFEST_DIR"));
    (
        std::fs::read(format!("{d}{name}.toml")).unwrap(),
        std::fs::read_to_string(format!("{d}{name}.sig.toml")).unwrap(),
    )
}

fn rejected<T: std::fmt::Debug>(r: Result<T, StationError>) -> bool {
    matches!(r, Err(StationError::Rejected(_)))
}

/// R-SEC-05, F-GND-07: a signed ruleset is accepted for the next start; a changed byte, another
/// item's signature or an unknown kind is refused, and nothing is stored for them.
#[test]
fn r_sec_05_uplinked_items_need_a_valid_signature() {
    let (_d, mut s) = station();
    let ruleset = caelum_alarms::BUNDLED_RULESET.as_bytes();
    let sigs = caelum_alarms::BUNDLED_SIGNATURES;
    let r = s.uplink_apply("ruleset", ruleset, sigs).unwrap();
    assert!(r.at_next_start);
    let tampered = caelum_alarms::BUNDLED_RULESET.replacen('9', "8", 1);
    assert!(rejected(s.uplink_apply(
        "ruleset",
        tampered.as_bytes(),
        sigs
    )));

    let (crew, crew_sig) = fixture("roster", "crew-v1");
    assert_eq!(
        s.uplink_apply("crew", &crew, &crew_sig).unwrap().version,
        "1"
    );
    let (_, other_sig) = fixture("roster", "crew-v2");
    assert!(rejected(s.uplink_apply("crew", &crew, &other_sig)));
    // A crew roster's signature does not make it a cluster configuration.
    assert!(rejected(s.uplink_apply("cluster", &crew, &crew_sig)));
    assert!(rejected(s.uplink_apply("firmware", &crew, &crew_sig)));

    let (cluster, cluster_sig) = fixture("cluster", "cluster-v1");
    assert!(
        s.uplink_apply("cluster", &cluster, &cluster_sig)
            .unwrap()
            .at_next_start
    );

    let uplinks = s
        .db()
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|e| e.target.as_deref() == Some("uplink"))
        .count();
    assert_eq!(uplinks, 3, "each accepted uplink is audited once");
}

/// R-SEC-05: only an administrator may apply an uplink.
#[test]
fn r_sec_05_only_an_administrator_applies_uplinks() {
    let (_d, mut s) = station();
    s.user_create("fs.one", Role::FlightSurgeon, None, ADMIN)
        .unwrap();
    s.logout().unwrap();
    s.login("fs.one", ADMIN.as_bytes()).unwrap();
    let (crew, crew_sig) = fixture("roster", "crew-v1");
    assert!(matches!(
        s.uplink_apply("crew", &crew, &crew_sig),
        Err(StationError::Denied(d)) if d.reason == DeniedReason::Role
    ));
}
