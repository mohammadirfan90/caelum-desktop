//! Technical alarms (F-ALM-06; SR-004, SR-012, R-OPS-04; CR-011): low battery, device lost, stale
//! data, clock drift, storage low. Sans-I/O: facts in, raise/clear changes out; the caller keeps
//! one technical alarm per (rule, crew member) and resolves it when its cause clears.
//!
//! Sensor off is reported by devices as an unusable signal (CR-008) and raises the SR-005 signal
//! alarm in the engine; the ruleset-invalid alarm is raised at load (SR-009).

use std::collections::BTreeMap;

use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use serde_json::json;

const NS_PER_S: i64 = 1_000_000_000;

/// Engineering limits (CR-011).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Battery below this percentage raises low battery.
    pub low_battery_pct: f64,
    /// A device disconnected this long is lost, s.
    pub device_lost_s: u32,
    /// Clock residual above this raises clock drift (R-OPS-04: 50 ms), ns.
    pub max_clock_residual_ns: u64,
    /// Free storage below this raises storage low, bytes.
    pub storage_low_bytes: u64,
}

/// A technical alarm to raise (with its why) or clear.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// Rule ID (`technical.*`).
    pub rule_id: String,
    /// Crew member, for crew-specific causes.
    pub crew: Option<CrewMemberId>,
    /// `Some(why)` raises, `None` clears.
    pub raise: Option<serde_json::Value>,
}

/// Facts seen so far.
#[derive(Debug, Default)]
pub struct Monitor {
    limits: Limits,
    last: BTreeMap<(CrewMemberId, Parameter), i64>,
    stale: BTreeMap<(CrewMemberId, Parameter), bool>,
    disconnected: BTreeMap<DeviceId, (Option<CrewMemberId>, i64, bool)>,
    raised: BTreeMap<String, bool>,
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Limits {
    /// CR-011 values.
    pub const DEFAULT: Self = Self {
        low_battery_pct: 20.0,
        device_lost_s: 30,
        max_clock_residual_ns: 50_000_000,
        storage_low_bytes: 1 << 30,
    };
}

impl Monitor {
    /// A monitor with these limits.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    fn set(
        &mut self,
        rule_id: String,
        crew: Option<CrewMemberId>,
        raise: Option<serde_json::Value>,
    ) -> Option<Change> {
        let key = format!(
            "{rule_id}/{}",
            crew.map(|c| c.to_string()).unwrap_or_default()
        );
        let now_raised = raise.is_some();
        let was = self.raised.insert(key, now_raised).unwrap_or(false);
        (was != now_raised).then_some(Change {
            rule_id,
            crew,
            raise,
        })
    }

    /// A stored observation: refreshes staleness, and checks battery levels.
    pub fn observation(
        &mut self,
        crew: CrewMemberId,
        device: Option<DeviceId>,
        parameter: Parameter,
        value: Option<f64>,
        at: TaiNs,
    ) -> Vec<Change> {
        let mut out = Vec::new();
        self.last.insert((crew, parameter), at.0);
        if self.stale.insert((crew, parameter), false) == Some(true) {
            out.extend(self.set(
                format!("technical.stale.{}", parameter.spec().key),
                Some(crew),
                None,
            ));
        }
        if let (Parameter::BatteryLevel, Some(v), Some(d)) = (parameter, value, device) {
            let low = v < self.limits.low_battery_pct;
            let why = low.then(|| json!({ "device": d.to_string(), "battery_pct": v, "limit_pct": self.limits.low_battery_pct }));
            out.extend(self.set(format!("technical.low_battery.{d}"), Some(crew), why));
        }
        out
    }

    /// Link state of a device (F-ACQ-10): lost after `device_lost_s` disconnected.
    pub fn link(
        &mut self,
        device: DeviceId,
        crew: Option<CrewMemberId>,
        connected: bool,
        at: TaiNs,
    ) -> Vec<Change> {
        if connected {
            self.disconnected.remove(&device);
            return self
                .set(format!("technical.device_lost.{device}"), crew, None)
                .into_iter()
                .collect();
        }
        self.disconnected
            .entry(device)
            .or_insert((crew, at.0, false));
        Vec::new()
    }

    /// A device clock offset (R-OPS-04) or a clock fault (SR-012: time going backwards).
    pub fn clock(
        &mut self,
        device: Option<DeviceId>,
        residual_ns: Option<u64>,
        backwards: bool,
    ) -> Vec<Change> {
        let who = device.map_or_else(|| "station".to_owned(), |d| d.to_string());
        let bad_residual = residual_ns.filter(|r| *r > self.limits.max_clock_residual_ns);
        let why = (backwards || bad_residual.is_some()).then(|| {
            json!({ "clock": who, "residual_ms": bad_residual.map(|r| r as f64 / 1e6), "time_went_backwards": backwards, "limit_ms": self.limits.max_clock_residual_ns as f64 / 1e6 })
        });
        self.set(format!("technical.clock.{who}"), None, why)
            .into_iter()
            .collect()
    }

    /// Free storage (F-SYS-09 watchdog).
    pub fn storage(&mut self, free_bytes: u64) -> Vec<Change> {
        let low = free_bytes < self.limits.storage_low_bytes;
        let why = low.then(
            || json!({ "free_bytes": free_bytes, "limit_bytes": self.limits.storage_low_bytes }),
        );
        self.set("technical.storage_low".to_owned(), None, why)
            .into_iter()
            .collect()
    }

    /// Periodic check: stale data (SR-004: never shown as normal) and lost devices.
    pub fn tick(&mut self, now: TaiNs) -> Vec<Change> {
        let mut out = Vec::new();
        let due: Vec<(CrewMemberId, Parameter, i64)> = self
            .last
            .iter()
            .filter(|((c, p), t)| {
                now.0 - **t > i64::from(p.spec().stale_after_s) * NS_PER_S
                    && self.stale.get(&(*c, *p)) != Some(&true)
            })
            .map(|((c, p), t)| (*c, *p, *t))
            .collect();
        for (crew, p, last) in due {
            self.stale.insert((crew, p), true);
            let why = json!({ "parameter": p.spec().key, "last_at_tai": last, "stale_after_s": p.spec().stale_after_s });
            out.extend(self.set(
                format!("technical.stale.{}", p.spec().key),
                Some(crew),
                Some(why),
            ));
        }
        let lost: Vec<(DeviceId, Option<CrewMemberId>, i64)> = self
            .disconnected
            .iter()
            .filter(|(_, (_, since, raised))| {
                !raised && now.0 - since >= i64::from(self.limits.device_lost_s) * NS_PER_S
            })
            .map(|(d, (c, since, _))| (*d, *c, *since))
            .collect();
        for (d, crew, since) in lost {
            if let Some(entry) = self.disconnected.get_mut(&d) {
                entry.2 = true;
            }
            let why = json!({ "device": d.to_string(), "disconnected_since_tai": since });
            out.extend(self.set(format!("technical.device_lost.{d}"), crew, Some(why)));
        }
        out
    }
}
