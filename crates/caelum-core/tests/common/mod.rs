//! Shared harness for the station tests: in-memory keystore, controllable clock, temporary data
//! directory.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)] // test code; not every test uses every item

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use caelum_core::security::clock::{Clock, ClockError};
use caelum_core::security::keystore::{KeyStore, KeyStoreError};
use caelum_core::security::role::Role;
use caelum_core::security::session::SessionConfig;
use caelum_core::station::{Station, StationError};
use caelum_domain::time::TaiNs;
use zeroize::Zeroizing;

#[derive(Default)]
pub struct MemoryKeyStore(Mutex<HashMap<String, Vec<u8>>>);

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
pub struct Times {
    pub tai: AtomicI64,
    pub mono: AtomicU64,
}

#[derive(Clone, Default)]
pub struct TestClock(pub Arc<Times>);

impl TestClock {
    #[allow(clippy::cast_possible_wrap)]
    pub fn advance_s(&self, s: u64) {
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
pub struct DataDir(pub PathBuf);

impl DataDir {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("caelum-core-{}", uuid::Uuid::new_v4()));
        Self(dir)
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn open(
    dir: &DataDir,
    keys: &MemoryKeyStore,
    clock: &TestClock,
) -> Result<Station, StationError> {
    Station::open(
        &dir.0,
        keys,
        Box::new(clock.clone()),
        Some("station-a".to_owned()),
        SessionConfig::default(),
    )
}

pub fn secret_for(role: Role) -> &'static str {
    if role.is_onboard_crew() {
        "246813"
    } else {
        "ground station passphrase"
    }
}
