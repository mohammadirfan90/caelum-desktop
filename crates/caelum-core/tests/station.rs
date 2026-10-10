//! M1 gate: role-matrix tests for every command through the station (F-SYS-01, R-SEC-02), and
//! key handling on first and later starts (F-SYS-02, R-SEC-01).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use caelum_core::security::audit::Actor;
use caelum_core::security::clock::{Clock, ClockError};
use caelum_core::security::keys::{DB_KEY_ENTRY, INSTANCE_KEY_ENTRY};
use caelum_core::security::keystore::{KeyStore, KeyStoreError};
use caelum_core::security::role::{Access, Command, DeniedReason, Role};
use caelum_core::security::session::{AuthError, SessionConfig};
use caelum_core::security::users::{create_user, NewUser};
use caelum_core::security::SecurityError;
use caelum_core::station::{Station, StationError, DATABASE_FILE};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::TaiNs;
use zeroize::Zeroizing;

#[derive(Default)]
struct MemoryKeyStore(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for MemoryKeyStore {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .map(Zeroizing::new))
    }
    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .unwrap()
            .insert(name.to_owned(), secret.to_vec());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), KeyStoreError> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

#[derive(Default)]
struct Times {
    tai: AtomicI64,
    mono: AtomicU64,
}

#[derive(Clone, Default)]
struct TestClock(Arc<Times>);

impl TestClock {
    #[allow(clippy::cast_possible_wrap)]
    fn advance_s(&self, s: u64) {
        self.0.mono.fetch_add(s * 1_000_000_000, Ordering::SeqCst);
        self.0
            .tai
            .fetch_add(i64::try_from(s).unwrap() * 1_000_000_000, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_tai(&self) -> Result<TaiNs, ClockError> {
        Ok(TaiNs(self.0.tai.load(Ordering::SeqCst)))
    }
    fn monotonic_ns(&self) -> u64 {
        self.0.mono.load(Ordering::SeqCst)
    }
}

/// A data directory removed on drop.
struct DataDir(PathBuf);

impl DataDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("caelum-core-{}", uuid::Uuid::new_v4()));
        Self(dir)
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &DataDir, keys: &MemoryKeyStore, clock: &TestClock) -> Result<Station, StationError> {
    Station::open(
        &dir.0,
        keys,
        Box::new(clock.clone()),
        Some("station-a".to_owned()),
        SessionConfig::default(),
    )
}

fn secret_for(role: Role) -> &'static str {
    if role.is_onboard_crew() {
        "246813"
    } else {
        "ground station passphrase"
    }
}

fn add_user(station: &Station, username: &str, role: Role) {
    let crew = role.is_onboard_crew().then(|| {
        let id = CrewMemberId::from_uuid(uuid::Uuid::new_v4());
        station
            .db()
            .connection()
            .execute(
                "INSERT INTO crew_member (id, name, role, mission, active) VALUES (?1, ?2, 'crew', 'M-1', 1)",
                (id.to_string(), username),
            )
            .unwrap();
        id
    });
    create_user(
        &station.log(),
        &Actor::system(),
        NewUser {
            username,
            role,
            crew_member_id: crew,
            secret: secret_for(role),
        },
    )
    .unwrap();
}

fn denied(r: Result<impl std::fmt::Debug, StationError>) -> Option<DeniedReason> {
    match r {
        Err(StationError::Denied(d)) => Some(d.reason),
        _ => None,
    }
}

/// Calls every M1 command except sign-in as the current caller; returns which were refused.
fn refused_commands(station: &mut Station, secret: &str) -> Vec<Command> {
    let mut refused = Vec::new();
    let mut check = |command, r: Option<DeniedReason>| {
        if r.is_some() {
            refused.push(command);
        }
    };
    check(Command::AuthMe, denied(station.me()));
    check(
        Command::AuthReauth,
        denied(station.reauth(secret.as_bytes())),
    );
    check(Command::AuditList, denied(station.audit_list()));
    check(Command::AuditVerify, denied(station.audit_verify()));
    check(Command::AuthLogout, denied(station.logout()));
    refused
}

fn expected_refusals(role: Option<Role>) -> Vec<Command> {
    [
        Command::AuthMe,
        Command::AuthReauth,
        Command::AuditList,
        Command::AuditVerify,
        Command::AuthLogout,
    ]
    .into_iter()
    .filter(|c| match (c.access(), role) {
        (Access::Public, _) => false,
        (Access::AnySignedIn, Some(_)) => false,
        (Access::Roles(roles), Some(r)) => !roles.contains(&r),
        (_, None) => true,
    })
    .collect()
}

/// Every command, for every role that can sign in, and without sign-in.
#[test]
fn every_command_is_authorised_per_role_matrix() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    let mut station = open(&dir, &keys, &clock).unwrap();
    for role in Role::ALL.into_iter().filter(|r| !r.is_machine()) {
        let name = format!("user.{}", role.as_str());
        add_user(&station, &name, role);
        let me = station.login(&name, secret_for(role).as_bytes()).unwrap();
        assert_eq!(me.identity.role, role);
        assert_eq!(
            refused_commands(&mut station, secret_for(role)),
            expected_refusals(Some(role)),
            "{role}"
        );
    }
    // Signed out (the last loop step signed out): everything but the public commands refused.
    assert_eq!(
        refused_commands(&mut station, "anything"),
        expected_refusals(None)
    );
    assert_eq!(
        denied(station.audit_list()),
        Some(DeniedReason::SignInRequired)
    );
    // Break-glass is public and always granted.
    assert!(station.break_glass_open().audit_error.is_none());
    assert_eq!(denied(station.me()), Some(DeniedReason::SignInRequired));
}

#[test]
fn machine_identities_cannot_sign_in_interactively() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let mut station = open(&dir, &keys, &TestClock::default()).unwrap();
    add_user(&station, "gateway", Role::DeviceGateway);
    assert!(matches!(
        station.login("gateway", secret_for(Role::DeviceGateway).as_bytes()),
        Err(StationError::Auth(AuthError::NotInteractive))
    ));
}

/// A locked session reaches only identity, unlock and sign-out (ROLE_MATRIX.md §4).
#[test]
fn a_locked_session_must_unlock_before_role_commands() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    let mut station = open(&dir, &keys, &clock).unwrap();
    add_user(&station, "cmo", Role::Cmo);
    station
        .login("cmo", secret_for(Role::Cmo).as_bytes())
        .unwrap();
    clock.advance_s(10 * 60);
    assert!(station.me().unwrap().locked);
    assert_eq!(denied(station.audit_list()), Some(DeniedReason::Locked));
    station.reauth(secret_for(Role::Cmo).as_bytes()).unwrap();
    assert!(!station.me().unwrap().locked);
    assert!(!station.audit_list().unwrap().is_empty());
}

/// Each role sees only its audit categories (ROLE_MATRIX.md §2); every entry so far is a security
/// entry, so the BME sees none and the administrator and CMO see all of them.
#[test]
fn audit_list_is_filtered_by_role() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let mut station = open(&dir, &keys, &TestClock::default()).unwrap();
    for (name, role) in [
        ("bme", Role::Bme),
        ("admin", Role::Administrator),
        ("cmo", Role::Cmo),
    ] {
        add_user(&station, name, role);
    }
    let mut visible = HashMap::new();
    for (name, role) in [
        ("bme", Role::Bme),
        ("admin", Role::Administrator),
        ("cmo", Role::Cmo),
    ] {
        station.login(name, secret_for(role).as_bytes()).unwrap();
        visible.insert(name, station.audit_list().unwrap().len());
    }
    assert_eq!(visible["bme"], 0);
    assert!(visible["admin"] > 0);
    assert!(visible["cmo"] >= visible["admin"]);
    assert!(station.audit_verify().unwrap().is_intact());
}

/// F-SYS-02: a new installation creates both keys (audited); later starts reuse them; an
/// existing database never gets a fresh key.
#[test]
fn keys_are_created_on_first_start_only() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    {
        let station = open(&dir, &keys, &clock).unwrap();
        let created: Vec<_> = station
            .db()
            .audit_records()
            .unwrap()
            .into_iter()
            .filter(|r| r.action == "key.create")
            .filter_map(|r| r.target)
            .collect();
        assert_eq!(created, [DB_KEY_ENTRY, INSTANCE_KEY_ENTRY]);
    }
    {
        let station = open(&dir, &keys, &clock).unwrap();
        assert_eq!(
            station.db().audit_records().unwrap().len(),
            2,
            "no new keys"
        );
        assert!(station.log().verify().unwrap().is_intact());
    }
    assert!(dir.0.join(DATABASE_FILE).exists());
    let empty = MemoryKeyStore::default();
    assert!(matches!(
        open(&dir, &empty, &clock),
        Err(StationError::Security(SecurityError::KeyMissing(
            DB_KEY_ENTRY
        )))
    ));
    assert!(
        empty.get(DB_KEY_ENTRY).unwrap().is_none(),
        "nothing created"
    );
}

/// SR-009: the station loads the signed ruleset at start-up; development builds run the
/// development-signed ruleset and report it for the DEVELOPMENT RULESET banner.
#[test]
fn sr_009_the_station_loads_the_ruleset_for_its_build() {
    use caelum_core::alarms::RulesetStatus;
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let station = open(&dir, &keys, &TestClock::default()).unwrap();
    assert!(
        matches!(
            station.ruleset_status(),
            RulesetStatus::Verified {
                development: true,
                ..
            }
        ),
        "{:?}",
        station.ruleset_status()
    );
}

fn crew_of_user(station: &Station, username: &str) -> CrewMemberId {
    station
        .db()
        .connection()
        .query_row(
            "SELECT crew_member_id FROM user WHERE username = ?1",
            [username],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
        .parse()
        .unwrap()
}

/// A critical SpO2 alarm for `crew`, raised through the alarm service.
fn critical_alarm(
    station: &mut Station,
    crew: CrewMemberId,
    t0: i64,
) -> caelum_domain::ids::AlarmId {
    use caelum_domain::observation::{Classification, MeasurementMode, Quality};
    use caelum_domain::parameters::Parameter;
    let device = caelum_domain::ids::DeviceId::from_uuid(uuid::Uuid::new_v4());
    let mut id = None;
    for k in 0..=15 {
        let obs = caelum_ingest::Observation {
            crew_member_id: crew,
            device_id: device,
            parameter: Parameter::Spo2,
            value: caelum_ingest::Value::Number(80.0),
            unit: Parameter::Spo2.spec().unit,
            measured_at: TaiNs(t0 + k * 1_000_000_000),
            received_at: TaiNs(t0 + k * 1_000_000_000),
            quality: Quality::Good,
            classification: Classification::Measured,
            mode: MeasurementMode::Continuous,
            provenance: caelum_ingest::Provenance {
                time: caelum_ingest::TimeSource::Receipt,
                original: None,
                site: None,
            },
        };
        let out = station.observe(&obs).unwrap();
        for ev in out.events {
            if let caelum_alarms::engine::Event::Raised(a) = ev {
                id = Some(a.id);
            }
        }
    }
    id.expect("alarm raised")
}

fn alarm_actions(station: &mut Station) -> Vec<String> {
    station
        .db()
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.action.starts_with("alarm."))
        .map(|r| r.action)
        .collect()
}

/// F-ALM-05, ROLE_MATRIX §2 and §5: astronauts act on their own alarms, the CMO on anyone's, the
/// flight surgeon acknowledges the ground stage only, others cannot act; every action is audited.
#[test]
fn alarm_actions_follow_the_role_matrix_and_are_audited() {
    use caelum_core::station::Acknowledged;
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    let mut station = open(&dir, &keys, &clock).unwrap();
    for (name, role) in [
        ("astro.a", Role::Astronaut),
        ("astro.b", Role::Astronaut),
        ("cmo", Role::Cmo),
        ("fs", Role::FlightSurgeon),
        ("bme", Role::Bme),
        ("admin", Role::Administrator),
    ] {
        add_user(&station, name, role);
    }
    let a = crew_of_user(&station, "astro.a");
    let alarm_a = critical_alarm(&mut station, a, 1_000_000_000_000);

    // Another astronaut: not their alarm.
    station
        .login("astro.b", secret_for(Role::Astronaut).as_bytes())
        .unwrap();
    assert_eq!(
        denied(station.alarm_acknowledge(alarm_a)),
        Some(DeniedReason::Role)
    );
    assert_eq!(
        denied(station.alarm_silence(alarm_a, 60)),
        Some(DeniedReason::Role)
    );
    // BME and administrator: no alarm actions at all.
    for (name, role) in [("bme", Role::Bme), ("admin", Role::Administrator)] {
        station.login(name, secret_for(role).as_bytes()).unwrap();
        assert_eq!(
            denied(station.alarm_acknowledge(alarm_a)),
            Some(DeniedReason::Role),
            "{name}"
        );
        assert_eq!(
            denied(station.alarm_escalate(alarm_a)),
            Some(DeniedReason::Role),
            "{name}"
        );
    }
    // Flight surgeon: ground stage only; cannot silence onboard.
    station
        .login("fs", secret_for(Role::FlightSurgeon).as_bytes())
        .unwrap();
    assert!(matches!(
        station.alarm_acknowledge(alarm_a).unwrap(),
        Acknowledged::GroundStage
    ));
    assert_eq!(
        denied(station.alarm_silence(alarm_a, 60)),
        Some(DeniedReason::Role)
    );
    // The astronaut silences (clamped) and escalates their own alarm.
    station
        .login("astro.a", secret_for(Role::Astronaut).as_bytes())
        .unwrap();
    assert_eq!(station.alarm_silence(alarm_a, 900).unwrap().1, 120);
    station.alarm_escalate(alarm_a).unwrap();
    // The CMO acknowledges it onboard.
    station
        .login("cmo", secret_for(Role::Cmo).as_bytes())
        .unwrap();
    assert!(matches!(
        station.alarm_acknowledge(alarm_a).unwrap(),
        Acknowledged::Onboard(_)
    ));
    assert_eq!(
        alarm_actions(&mut station),
        [
            "alarm.ground_acknowledge",
            "alarm.silence",
            "alarm.escalate",
            "alarm.acknowledge"
        ]
    );
    // Unknown alarm; signed out.
    let ghost = caelum_domain::ids::AlarmId::from_uuid(uuid::Uuid::new_v4());
    assert!(matches!(
        station.alarm_acknowledge(ghost),
        Err(StationError::Alarms(_))
    ));
    station.logout().unwrap();
    assert_eq!(
        denied(station.alarm_acknowledge(alarm_a)),
        Some(DeniedReason::SignInRequired)
    );
}

fn open_technical(station: &Station) -> Vec<String> {
    station
        .db()
        .open_alarm_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == "technical")
        .map(|r| r.rule_id)
        .collect()
}

/// M4 gate, F-SYS-09: a failed self-test raises a technical alarm; passing again clears it.
#[test]
fn f_sys_09_a_failed_self_test_raises_a_technical_alarm() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    clock.0.tai.store(
        2_100_000_000_000_000_000,
        std::sync::atomic::Ordering::SeqCst,
    );
    let mut station = open(&dir, &keys, &clock).unwrap();
    let report = station.self_test_report().to_vec();
    assert!(report.iter().all(|c| c.ok), "{report:?}");
    assert!(open_technical(&station).is_empty());

    // Storage below the limit (limit raised for the test) fails the storage check.
    let checks = station.run_self_test(u64::MAX).unwrap();
    assert!(!checks.iter().find(|c| c.name == "storage").unwrap().ok);
    assert_eq!(open_technical(&station), ["technical.self_test.storage"]);
    station.run_self_test(0).unwrap();
    assert!(
        open_technical(&station).is_empty(),
        "cleared when the check passes"
    );

    // A tampered audit chain fails the audit check.
    station
        .db()
        .connection()
        .execute_batch(
            "DROP TRIGGER audit_entry_no_update; UPDATE audit_entry SET actor = 'x' WHERE seq = 1;",
        )
        .unwrap();
    station.run_self_test(0).unwrap();
    assert_eq!(
        open_technical(&station),
        ["technical.self_test.audit_chain"]
    );
}

/// F-SYS-09: a clock that cannot give TAI fails the clock check (time marked unknown).
#[test]
fn f_sys_09_an_invalid_clock_fails_the_self_test() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let station = open(&dir, &keys, &TestClock::default()).unwrap();
    let clock = station
        .self_test_report()
        .iter()
        .find(|c| c.name == "clock")
        .unwrap()
        .clone();
    assert!(
        !clock.ok,
        "TAI 0 (1958) predates the leap-second table: {clock:?}"
    );
    assert!(open_technical(&station).contains(&"technical.self_test.clock".to_owned()));
}

/// docs/03 §3, F-SYS-09: the station tick escalates (sound on every crew station at 15 s), and its
/// watchdogs raise a clock fault when time goes backwards (SR-012) and stale data (SR-004).
#[test]
fn the_station_tick_escalates_and_watches_clock_and_staleness() {
    use caelum_alarms::escalation::{Notice, SoundScope};
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    let t0 = 2_100_000_000_000_000_000;
    clock.0.tai.store(t0, Ordering::SeqCst);
    let mut station = open(&dir, &keys, &clock).unwrap();
    add_user(&station, "astro.a", Role::Astronaut);
    let crew = crew_of_user(&station, "astro.a");
    let alarm = critical_alarm(&mut station, crew, t0);
    // Created at t0 + 15 s; the sound stage is due 15 s later.
    clock.advance_s(30);
    let out = station.tick().unwrap();
    assert!(
        out.notices.iter().any(|n| matches!(n, Notice::SoundOn { alarm: a, scope: SoundScope::AllCrewStations, wristband: true, .. } if *a == alarm)),
        "{:?}",
        out.notices
    );
    // SpO2 last measured at t0 + 15 s; stale_after_s later the value is stale.
    clock.advance_s(i64::from(
        caelum_domain::parameters::Parameter::Spo2
            .spec()
            .stale_after_s,
    ) as u64);
    station.tick().unwrap();
    assert!(
        open_technical(&station).contains(&"technical.stale.spo2".to_owned()),
        "{:?}",
        open_technical(&station)
    );
    // The wall clock steps back an hour: clock fault (SR-012).
    clock
        .0
        .tai
        .fetch_sub(3_600 * 1_000_000_000, Ordering::SeqCst);
    station.tick().unwrap();
    assert!(open_technical(&station).contains(&"technical.clock.station".to_owned()));
    clock.advance_s(1);
    station.tick().unwrap();
    assert!(
        !open_technical(&station).contains(&"technical.clock.station".to_owned()),
        "clears when time moves forward again"
    );
}

/// F-ALM-04, WX-002, SR-015: the wristband alert goes only to wristbands assigned to the alarm's
/// crew member; with none, there is no target and the crew stations still sound.
#[test]
fn wristband_alerts_target_only_the_crew_members_wristbands() {
    use caelum_devices::registry::{DeviceIdentity, Registry};
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    let mut station = open(&dir, &keys, &clock).unwrap();
    add_user(&station, "astro.a", Role::Astronaut);
    add_user(&station, "astro.b", Role::Astronaut);
    let a = crew_of_user(&station, "astro.a");
    let b = crew_of_user(&station, "astro.b");
    let alarm = critical_alarm(&mut station, a, 1_000_000_000_000);
    assert!(
        station.wristband_targets(alarm).unwrap().is_empty(),
        "no wristband: no target"
    );
    let reg = Registry::new(station.db());
    let identity = |adapter: &str, serial: &str| DeviceIdentity {
        adapter: adapter.into(),
        vendor: "V".into(),
        model: "M".into(),
        serial: serial.into(),
        firmware: None,
        accuracy_class: None,
        calibration_due: None,
    };
    let mine = reg
        .register(&identity(caelum_devices::ble::WRISTBAND_ADAPTER, "W-A"))
        .unwrap();
    let theirs = reg
        .register(&identity(caelum_devices::ble::WRISTBAND_ADAPTER, "W-B"))
        .unwrap();
    let strap = reg.register(&identity("ble-sig", "HR-A")).unwrap();
    reg.assign(mine, a, None).unwrap();
    reg.assign(theirs, b, None).unwrap();
    reg.assign(strap, a, None).unwrap();
    assert_eq!(
        station.wristband_targets(alarm).unwrap(),
        [mine.to_string()]
    );
}
