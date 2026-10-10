//! The deterministic rule engine (F-ALM-01/02/03/08; SR-001..006, SR-010, SR-018, SR-019).
//!
//! Sans-I/O: it receives observations, events and ticks with their TAI times and returns alarm
//! events; the caller persists and distributes them. Given the same inputs it produces the same
//! outputs. It runs synchronously in the ingest path (docs/03 §3).
//!
//! - Continuous readings (SR-003, SR-004): a rule fires after its persistence window and clears
//!   after its clear threshold held for its clear window; a gap longer than `max_gap_s` restarts
//!   timing.
//! - Spot readings (SR-019): the first breach opens `CONFIRMING`; any breaching repeat, or no
//!   repeat within `recheck_timeout_s`, raises the full alarm; a non-breaching repeat closes it as
//!   not confirmed with both values; a breach within `confirm_window_s` of an earlier
//!   not-confirmed breach raises the full alarm at once (CR-009 §3).
//! - Quality (SR-005): POOR and UNUSABLE readings pause that signal's rules and raise a technical
//!   event; nothing is evaluated from them.
//! - Context (SR-018): "at rest" rules are evaluated when activity is unknown or stale, and the why
//!   says so; known activity above the limit means the rule does not apply.
//! - Supersession (SR-006): within a group a higher priority closes lower open alarms; a lower
//!   alarm is not opened while an equal or higher one is open, and opens later if still breaching.
//! - Absolute limits are never touched by baselines (SR-001, SR-002): baseline notices are
//!   observe-level and cannot supersede them.

use std::collections::{BTreeMap, HashMap, VecDeque};

use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::observation::{MeasurementMode, Quality, SiteClass};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use serde::Serialize;

use crate::ruleset::{
    Condition, Limits, Op, Priority, Rule, Ruleset, SpotConfirm, Threshold, ThresholdLimits,
};

const NS_PER_S: i64 = 1_000_000_000;

fn secs(s: u32) -> i64 {
    i64::from(s) * NS_PER_S
}

/// One numeric observation to evaluate (from ingest).
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    /// Crew member.
    pub crew: CrewMemberId,
    /// Source device, if any.
    pub device: Option<DeviceId>,
    /// Parameter.
    pub parameter: Parameter,
    /// Value in the parameter's canonical unit.
    pub value: f64,
    /// Measurement time.
    pub at: TaiNs,
    /// Quality.
    pub quality: Quality,
    /// Continuous or spot.
    pub mode: MeasurementMode,
    /// Thermometer site class, for `body_temp_measured`.
    pub site: Option<SiteClass>,
}

/// A personal baseline (median and MAD) supplied by the caller for baseline notices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaselineStats {
    /// Median.
    pub median: f64,
    /// Median absolute deviation.
    pub mad: f64,
    /// Samples behind it.
    pub n: u32,
}

/// Alarm state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlarmState {
    /// A breaching spot reading awaits confirmation (shown as an alarm, SR-019).
    Confirming,
    /// Condition present.
    Active,
    /// Condition cleared; latched until acknowledged.
    ClearedAwaitingAck,
}

/// One reading behind an alarm.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WhyReading {
    /// Value.
    pub value: f64,
    /// Time (TAI ns).
    pub at: i64,
    /// Quality name.
    pub quality: &'static str,
    /// Spot or continuous.
    pub mode: &'static str,
    /// Device.
    pub device: Option<String>,
}

/// Why an alarm fired (SR-010).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Why {
    /// Rule ID.
    pub rule_id: String,
    /// Ruleset version.
    pub ruleset_version: String,
    /// Parameter key.
    pub parameter: &'static str,
    /// UCUM unit.
    pub unit: &'static str,
    /// Threshold crossed (the one for the reading's site, if site-dependent).
    pub threshold: Option<f64>,
    /// Direction (`<` or `>`).
    pub op: Option<&'static str>,
    /// How long the condition held, s.
    pub held_s: Option<f64>,
    /// The readings, oldest first.
    pub readings: Vec<WhyReading>,
    /// Further facts (e.g. "activity unknown", "no repeat reading within 120 s").
    pub notes: Vec<String>,
}

/// An alarm to restore after a restart (from the stored record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// ID.
    pub id: AlarmId,
    /// Crew member.
    pub crew: CrewMemberId,
    /// Rule.
    pub rule_id: String,
    /// State.
    pub state: AlarmState,
    /// Acknowledged.
    pub acknowledged: bool,
    /// Creation (TAI ns).
    pub created_at: i64,
}

/// An open alarm.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Alarm {
    /// ID.
    pub id: AlarmId,
    /// Crew member.
    pub crew: CrewMemberId,
    /// Rule ID.
    pub rule_id: String,
    /// Priority.
    pub priority: &'static str,
    /// Category.
    pub category: &'static str,
    /// State.
    pub state: AlarmState,
    /// Latched until acknowledged.
    pub latched: bool,
    /// Value that triggered it.
    pub trigger_value: Option<f64>,
    /// Title.
    pub title: String,
    /// Action.
    pub action: String,
    /// Why.
    pub why: Why,
    /// Created (TAI ns).
    pub created_at: i64,
}

/// What the engine reports.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A spot reading opened a confirming alarm (shown, re-measure prompt, CMO notified).
    Confirming(Alarm),
    /// A full alarm (new, or confirmed from `CONFIRMING`).
    Raised(Alarm),
    /// A confirming alarm closed: the repeat reading did not breach (logged with both values).
    NotConfirmed(Alarm),
    /// The condition cleared; the alarm stays until acknowledged (latching).
    ClearedAwaitingAck(Alarm),
    /// A latched alarm whose condition had cleared is breaching again before acknowledgement:
    /// active again at once (it never ended, so persistence does not apply again).
    Reactivated(Alarm),
    /// The alarm ended.
    Resolved {
        /// Alarm.
        alarm: AlarmId,
        /// Why it ended.
        reason: String,
    },
    /// A higher-priority alarm in the same group replaced it (SR-006).
    Superseded {
        /// The lower alarm.
        alarm: AlarmId,
        /// The higher one.
        by: AlarmId,
    },
    /// A signal's quality pauses its rules (SR-005).
    SignalUnusable {
        /// Crew member.
        crew: CrewMemberId,
        /// Device, if known.
        device: Option<DeviceId>,
        /// Parameter.
        parameter: Parameter,
        /// Quality that paused it.
        quality: Quality,
    },
    /// The signal is usable again.
    SignalRecovered {
        /// Crew member.
        crew: CrewMemberId,
        /// Device.
        device: Option<DeviceId>,
        /// Parameter.
        parameter: Parameter,
    },
}

#[derive(Debug, Clone, Copy)]
struct Timer {
    start: i64,
    last: i64,
}

/// Starts or extends a timer; a gap longer than `max_gap` restarts it (SR-004). Returns the start.
fn advance(timer: &mut Option<Timer>, t: i64, max_gap: i64) -> i64 {
    match timer {
        Some(tm) if t - tm.last <= max_gap => {
            if t > tm.last {
                tm.last = t;
            }
            tm.start
        }
        _ => {
            *timer = Some(Timer { start: t, last: t });
            t
        }
    }
}

#[derive(Debug, Clone)]
struct OpenAlarm {
    alarm: Alarm,
    priority: Priority,
    group: String,
    acknowledged: bool,
    /// For `CONFIRMING`: when the recheck timeout falls due.
    recheck_due: Option<i64>,
}

#[derive(Debug, Default, Clone)]
struct RuleState {
    pending: Option<Timer>,
    clearing: Option<Timer>,
    open: Option<AlarmId>,
    /// Last breaching spot reading closed as not confirmed (value, time).
    unconfirmed: Option<(f64, i64)>,
    /// Spot readings for k-of-n baselines (deviating?).
    spot_window: VecDeque<bool>,
    /// Absence rules: since when the gate has been valid.
    gate_open_since: Option<i64>,
    /// Absence rules: last event.
    last_event: Option<i64>,
    /// Absence rules: events seen continuously since.
    events_since: Option<i64>,
}

/// Engine state for one ruleset.
pub struct Engine {
    ruleset: Ruleset,
    state: HashMap<(CrewMemberId, String), RuleState>,
    open: BTreeMap<AlarmId, OpenAlarm>,
    activity: HashMap<CrewMemberId, (f64, i64)>,
    gates: HashMap<(CrewMemberId, Parameter), (f64, i64)>,
    paused: HashMap<(CrewMemberId, Option<DeviceId>, Parameter), Quality>,
    new_id: Box<dyn FnMut() -> AlarmId + Send>,
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Good => "good",
        Quality::Degraded => "degraded",
        Quality::Poor => "poor",
        Quality::Unusable => "unusable",
    }
}

fn mode_name(m: MeasurementMode) -> &'static str {
    match m {
        MeasurementMode::Continuous => "continuous",
        MeasurementMode::Spot => "spot",
    }
}

fn op_name(op: Op) -> &'static str {
    match op {
        Op::Below => "<",
        Op::Above => ">",
    }
}

fn limits_for(t: &Threshold, site: Option<SiteClass>) -> Limits {
    match t.limits {
        ThresholdLimits::Fixed(l) => l,
        ThresholdLimits::BySite(s) => match site.unwrap_or(SiteClass::Unknown) {
            SiteClass::CoreEquivalent => s.core_equivalent,
            SiteClass::Peripheral => s.peripheral,
            SiteClass::Unknown => s.unknown,
        },
    }
}

impl Engine {
    /// An engine for a verified ruleset, with random alarm IDs.
    pub fn new(ruleset: Ruleset) -> Self {
        Self::with_ids(
            ruleset,
            Box::new(|| AlarmId::from_uuid(uuid::Uuid::new_v4())),
        )
    }

    /// An engine with a custom ID source (tests).
    pub fn with_ids(ruleset: Ruleset, new_id: Box<dyn FnMut() -> AlarmId + Send>) -> Self {
        Self {
            ruleset,
            state: HashMap::new(),
            open: BTreeMap::new(),
            activity: HashMap::new(),
            gates: HashMap::new(),
            paused: HashMap::new(),
            new_id,
        }
    }

    /// The ruleset in use.
    pub fn ruleset(&self) -> &Ruleset {
        &self.ruleset
    }

    /// Open alarms (any state), by ID.
    pub fn open_alarms(&self) -> impl Iterator<Item = &Alarm> {
        self.open.values().map(|o| &o.alarm)
    }

    fn reading(input: &Input) -> WhyReading {
        WhyReading {
            value: input.value,
            at: input.at.0,
            quality: quality_name(input.quality),
            mode: mode_name(input.mode),
            device: input.device.map(|d| d.to_string()),
        }
    }

    /// SR-018: activity is unknown when never reported or older than its staleness limit.
    fn activity_at(&self, crew: CrewMemberId, at: i64) -> Option<f64> {
        let (v, t) = *self.activity.get(&crew)?;
        let stale = secs(Parameter::ActivityLevel.spec().stale_after_s);
        (at - t <= stale && at >= t).then_some(v)
    }

    /// Evaluates one observation (and, for baseline parameters, its baseline).
    pub fn evaluate(&mut self, input: &Input, baseline: Option<BaselineStats>) -> Vec<Event> {
        let mut events = Vec::new();
        if !input.value.is_finite() {
            return events;
        }
        let key = (input.crew, input.device, input.parameter);
        if !input.quality.usable_for_rules() {
            // SR-005: an unusable reading of a gate parameter (e.g. `sqi_ecg` with the lead off)
            // closes the gate now; it must not keep an absence rule armed on the last good value.
            self.gates.remove(&(input.crew, input.parameter));
            if self.paused.insert(key, input.quality).is_none() {
                events.push(Event::SignalUnusable {
                    crew: input.crew,
                    device: input.device,
                    parameter: input.parameter,
                    quality: input.quality,
                });
            }
            return events;
        }
        if self.paused.remove(&key).is_some() {
            events.push(Event::SignalRecovered {
                crew: input.crew,
                device: input.device,
                parameter: input.parameter,
            });
        }
        if input.parameter == Parameter::ActivityLevel {
            self.activity.insert(input.crew, (input.value, input.at.0));
        }
        let rules: Vec<Rule> = self.ruleset.rules_for(input.parameter).cloned().collect();
        for rule in &rules {
            match &rule.condition {
                Condition::Threshold(t) => self.threshold(rule, t, input, &mut events),
                Condition::Absence(_) => {
                    self.gates
                        .insert((input.crew, input.parameter), (input.value, input.at.0));
                    self.absence(rule, input.crew, input.at.0, &mut events);
                }
                Condition::Baseline => {}
            }
        }
        if let Some(b) = baseline {
            self.baseline(input, b, &mut events);
        }
        events
    }

    /// An expected event (e.g. a detected QRS complex) for absence rules.
    pub fn event(&mut self, crew: CrewMemberId, name: &str, at: TaiNs) -> Vec<Event> {
        let mut events = Vec::new();
        let rules: Vec<Rule> = self
            .ruleset
            .rules
            .iter()
            .filter(|r| matches!(&r.condition, Condition::Absence(a) if a.event == name))
            .cloned()
            .collect();
        for rule in &rules {
            let st = self.state.entry((crew, rule.id.clone())).or_default();
            let window = secs(rule.fire_for_s);
            let continuous = st.last_event.is_some_and(|l| at.0 - l < window);
            if !continuous || st.events_since.is_none() {
                st.events_since = Some(at.0);
            }
            st.last_event = Some(at.0);
            self.absence(rule, crew, at.0, &mut events);
        }
        events
    }

    /// The 4 Hz tick: absence windows and spot recheck timeouts.
    pub fn tick(&mut self, now: TaiNs) -> Vec<Event> {
        let mut events = Vec::new();
        let absence: Vec<Rule> = self
            .ruleset
            .rules
            .iter()
            .filter(|r| matches!(r.condition, Condition::Absence(_)))
            .cloned()
            .collect();
        let crews: Vec<CrewMemberId> = {
            let mut c: Vec<CrewMemberId> = self.gates.keys().map(|(c, _)| *c).collect();
            c.sort_unstable();
            c.dedup();
            c
        };
        for rule in &absence {
            for crew in &crews {
                self.absence(rule, *crew, now.0, &mut events);
            }
        }
        let due: Vec<AlarmId> = self
            .open
            .iter()
            .filter(|(_, o)| {
                o.alarm.state == AlarmState::Confirming && o.recheck_due.is_some_and(|d| now.0 >= d)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(o) = self.open.get_mut(&id) {
                let timeout = o.recheck_due.map_or(0, |d| d - o.alarm.created_at) / NS_PER_S;
                o.alarm.state = AlarmState::Active;
                o.recheck_due = None;
                o.alarm
                    .why
                    .notes
                    .push(format!("no repeat reading within {timeout} s"));
                events.push(Event::Raised(o.alarm.clone()));
            }
        }
        events
    }

    /// Restores an alarm that was open when CAELUM stopped (SR-007), so its rule continues it
    /// instead of raising a duplicate. Returns false if the rule is not in this ruleset (then the
    /// caller keeps it on record for acknowledgement only).
    pub fn restore(&mut self, r: &Restored) -> bool {
        let Some(rule) = self
            .ruleset
            .rules
            .iter()
            .find(|x| x.id == r.rule_id)
            .cloned()
        else {
            return false;
        };
        let recheck_due = match (&rule.condition, r.state) {
            (Condition::Threshold(t), AlarmState::Confirming) => {
                t.spot.map(|s| r.created_at + secs(s.recheck_timeout_s))
            }
            _ => None,
        };
        let why = self.why(
            &rule,
            None,
            None,
            None,
            Vec::new(),
            vec!["restored after a restart; values are in the stored record".to_owned()],
        );
        let alarm = Alarm {
            id: r.id,
            crew: r.crew,
            rule_id: rule.id.clone(),
            priority: rule.priority.as_str(),
            category: rule.category.as_str(),
            state: r.state,
            latched: rule.priority.latching(),
            trigger_value: None,
            title: rule.title.clone(),
            action: rule.action.clone(),
            why,
            created_at: r.created_at,
        };
        self.state
            .entry((r.crew, rule.id.clone()))
            .or_default()
            .open = Some(r.id);
        self.open.insert(
            r.id,
            OpenAlarm {
                alarm,
                priority: rule.priority,
                group: rule.group.clone(),
                acknowledged: r.acknowledged,
                recheck_due,
            },
        );
        true
    }

    /// Acknowledges an alarm: a latched alarm whose condition cleared ends; otherwise it stays
    /// until the condition clears (escalation and audit are M4, F-ALM-05).
    pub fn acknowledge(&mut self, id: AlarmId) -> Vec<Event> {
        let Some(o) = self.open.get_mut(&id) else {
            return Vec::new();
        };
        o.acknowledged = true;
        if o.alarm.state == AlarmState::ClearedAwaitingAck {
            return self.close(id, "acknowledged after the condition cleared");
        }
        Vec::new()
    }

    fn close(&mut self, id: AlarmId, reason: &str) -> Vec<Event> {
        if let Some(o) = self.open.remove(&id) {
            if let Some(st) = self.state.get_mut(&(o.alarm.crew, o.alarm.rule_id.clone())) {
                if st.open == Some(id) {
                    st.open = None;
                }
            }
            return vec![Event::Resolved {
                alarm: id,
                reason: reason.to_owned(),
            }];
        }
        Vec::new()
    }

    /// The condition cleared: latched alarms wait for acknowledgement, others end.
    fn condition_cleared(&mut self, id: AlarmId, reason: &str, events: &mut Vec<Event>) {
        let Some(o) = self.open.get_mut(&id) else {
            return;
        };
        if o.alarm.latched && !o.acknowledged {
            if o.alarm.state != AlarmState::ClearedAwaitingAck {
                o.alarm.state = AlarmState::ClearedAwaitingAck;
                events.push(Event::ClearedAwaitingAck(o.alarm.clone()));
            }
        } else {
            events.extend(self.close(id, reason));
        }
    }

    /// SR-006: whether an equal or higher open alarm in the group blocks a new one.
    fn blocked(&self, crew: CrewMemberId, group: &str, priority: Priority) -> bool {
        self.open
            .values()
            .any(|o| o.alarm.crew == crew && o.group == group && o.priority >= priority)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_alarm(
        &mut self,
        rule: &Rule,
        crew: CrewMemberId,
        state: AlarmState,
        trigger: Option<f64>,
        why: Why,
        at: i64,
        recheck_due: Option<i64>,
        events: &mut Vec<Event>,
    ) -> Option<AlarmId> {
        if self.blocked(crew, &rule.group, rule.priority) {
            return None;
        }
        let id = (self.new_id)();
        // SR-006: the new alarm supersedes lower ones in its group.
        let lower: Vec<AlarmId> = self
            .open
            .iter()
            .filter(|(_, o)| {
                o.alarm.crew == crew && o.group == rule.group && o.priority < rule.priority
            })
            .map(|(k, _)| *k)
            .collect();
        for l in lower {
            if let Some(o) = self.open.remove(&l) {
                if let Some(st) = self.state.get_mut(&(crew, o.alarm.rule_id.clone())) {
                    st.open = None;
                }
                events.push(Event::Superseded { alarm: l, by: id });
            }
        }
        let alarm = Alarm {
            id,
            crew,
            rule_id: rule.id.clone(),
            priority: rule.priority.as_str(),
            category: rule.category.as_str(),
            state,
            latched: rule.priority.latching(),
            trigger_value: trigger,
            title: rule.title.clone(),
            action: rule.action.clone(),
            why,
            created_at: at,
        };
        events.push(match state {
            AlarmState::Confirming => Event::Confirming(alarm.clone()),
            AlarmState::Active | AlarmState::ClearedAwaitingAck => Event::Raised(alarm.clone()),
        });
        self.open.insert(
            id,
            OpenAlarm {
                alarm,
                priority: rule.priority,
                group: rule.group.clone(),
                acknowledged: false,
                recheck_due,
            },
        );
        Some(id)
    }

    fn why(
        &self,
        rule: &Rule,
        threshold: Option<f64>,
        op: Option<Op>,
        held: Option<i64>,
        readings: Vec<WhyReading>,
        notes: Vec<String>,
    ) -> Why {
        let spec = rule.parameter.spec();
        Why {
            rule_id: rule.id.clone(),
            ruleset_version: self.ruleset.version.clone(),
            parameter: spec.key,
            unit: spec.unit.ucum(),
            threshold,
            op: op.map(op_name),
            held_s: held.map(|h| h as f64 / NS_PER_S as f64),
            readings,
            notes,
        }
    }

    fn threshold(&mut self, rule: &Rule, t: &Threshold, input: &Input, events: &mut Vec<Event>) {
        let at = input.at.0;
        let limits = limits_for(t, input.site);
        // SR-018: the context condition.
        let mut notes = Vec::new();
        let applies = match t.at_rest {
            None => true,
            Some((_, max)) => match self.activity_at(input.crew, at) {
                Some(a) => a <= max,
                None => {
                    notes.push("activity unknown: evaluated as if at rest (SR-018)".to_owned());
                    true
                }
            },
        };
        if let (ThresholdLimits::BySite(_), None | Some(SiteClass::Unknown)) =
            (t.limits, input.site)
        {
            notes.push("thermometer site unknown: most conservative threshold".to_owned());
        }
        let breached = applies && t.op.breached(input.value, limits.fire);
        let cleared = t.op.cleared(input.value, limits.clear);
        match input.mode {
            MeasurementMode::Spot => {
                self.spot(rule, t, limits, input, breached, cleared, notes, events)
            }
            MeasurementMode::Continuous => {
                self.continuous(rule, t.op, limits, input, breached, cleared, notes, events)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn continuous(
        &mut self,
        rule: &Rule,
        op: Op,
        limits: Limits,
        input: &Input,
        breached: bool,
        cleared: bool,
        notes: Vec<String>,
        events: &mut Vec<Event>,
    ) {
        let at = input.at.0;
        let gap = secs(rule.max_gap_s);
        let skey = (input.crew, rule.id.clone());
        let st = self.state.entry(skey.clone()).or_default();
        if breached {
            st.clearing = None;
            let start = advance(&mut st.pending, at, gap);
            let held = at - start;
            if let Some(id) = st.open {
                if let Some(o) = self.open.get_mut(&id) {
                    let reopen = match o.alarm.state {
                        // Breaching again after clearing: the condition is present again.
                        AlarmState::ClearedAwaitingAck => true,
                        // A spot-opened alarm confirmed by continuous data meeting persistence.
                        AlarmState::Confirming => held >= secs(rule.fire_for_s),
                        AlarmState::Active => false,
                    };
                    if reopen {
                        let was = o.alarm.state;
                        o.alarm.state = AlarmState::Active;
                        o.recheck_due = None;
                        o.alarm.why.readings.push(Self::reading(input));
                        events.push(if was == AlarmState::ClearedAwaitingAck {
                            Event::Reactivated(o.alarm.clone())
                        } else {
                            Event::Raised(o.alarm.clone())
                        });
                    }
                }
                return;
            }
            if held >= secs(rule.fire_for_s) {
                let why = self.why(
                    rule,
                    Some(limits.fire),
                    Some(op),
                    Some(held),
                    vec![Self::reading(input)],
                    notes,
                );
                let opened = self.open_alarm(
                    rule,
                    input.crew,
                    AlarmState::Active,
                    Some(input.value),
                    why,
                    at,
                    None,
                    events,
                );
                if let Some(st) = self.state.get_mut(&skey) {
                    st.open = opened;
                }
            }
            return;
        }
        st.pending = None;
        let Some(id) = st.open else {
            st.clearing = None;
            return;
        };
        if !cleared {
            st.clearing = None;
            return;
        }
        let start = advance(&mut st.clearing, at, gap);
        if at - start >= secs(rule.clear_for_s) {
            st.clearing = None;
            let reason = format!("{} back within limits", rule.parameter.spec().key);
            self.condition_cleared(id, &reason, events);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spot(
        &mut self,
        rule: &Rule,
        t: &Threshold,
        limits: Limits,
        input: &Input,
        breached: bool,
        cleared: bool,
        mut notes: Vec<String>,
        events: &mut Vec<Event>,
    ) {
        let at = input.at.0;
        let skey = (input.crew, rule.id.clone());
        let st = self.state.entry(skey.clone()).or_default();
        let open = st.open;
        let previous = st.unconfirmed;
        let confirm: Option<SpotConfirm> = t.spot;
        if breached {
            if let Some(id) = open {
                if let Some(o) = self.open.get_mut(&id) {
                    match o.alarm.state {
                        AlarmState::Confirming => {
                            o.alarm.state = AlarmState::Active;
                            o.recheck_due = None;
                            o.alarm.why.readings.push(Self::reading(input));
                            o.alarm
                                .why
                                .notes
                                .push("confirmed by a second breaching reading".to_owned());
                            events.push(Event::Raised(o.alarm.clone()));
                        }
                        AlarmState::ClearedAwaitingAck => {
                            o.alarm.state = AlarmState::Active;
                            o.alarm.why.readings.push(Self::reading(input));
                            events.push(Event::Reactivated(o.alarm.clone()));
                        }
                        AlarmState::Active => o.alarm.why.readings.push(Self::reading(input)),
                    }
                }
                return;
            }
            let mut readings = Vec::new();
            let (state, recheck) = match confirm {
                // No spot settings (derived, environment): never ignored, never delayed.
                None => {
                    notes.push(
                        "spot reading on a rule without spot confirmation: full alarm".to_owned(),
                    );
                    (AlarmState::Active, None)
                }
                Some(c) => match previous {
                    Some((v, pt)) if at - pt <= secs(c.confirm_window_s) => {
                        readings.push(WhyReading {
                            value: v,
                            at: pt,
                            quality: "unknown",
                            mode: "spot",
                            device: None,
                        });
                        notes.push("second breach within the confirmation window".to_owned());
                        (AlarmState::Active, None)
                    }
                    _ => (AlarmState::Confirming, Some(at + secs(c.recheck_timeout_s))),
                },
            };
            readings.push(Self::reading(input));
            let why = self.why(rule, Some(limits.fire), Some(t.op), None, readings, notes);
            let opened = self.open_alarm(
                rule,
                input.crew,
                state,
                Some(input.value),
                why,
                at,
                recheck,
                events,
            );
            if let Some(st) = self.state.get_mut(&skey) {
                st.open = opened;
                st.unconfirmed = None;
                if opened.is_none() {
                    // Blocked by a higher alarm in its group: remember it so a repeat confirms.
                    st.unconfirmed = Some((input.value, at));
                }
            }
            return;
        }
        let Some(id) = open else {
            return;
        };
        let Some(o) = self.open.get(&id) else {
            return;
        };
        match o.alarm.state {
            AlarmState::Confirming => {
                let mut alarm = o.alarm.clone();
                alarm.why.readings.push(Self::reading(input));
                alarm
                    .why
                    .notes
                    .push("repeat reading did not breach: not confirmed".to_owned());
                let first = alarm.why.readings.first().map(|r| (r.value, r.at));
                self.open.remove(&id);
                if let Some(st) = self.state.get_mut(&skey) {
                    st.open = None;
                    st.unconfirmed = first;
                }
                events.push(Event::NotConfirmed(alarm));
            }
            // CR-003 (proposed): a confirmed spot alarm clears on one reading meeting the clear
            // threshold; latching rules still need acknowledgement.
            AlarmState::Active if cleared => {
                let reason = format!("{} back within limits", rule.parameter.spec().key);
                self.condition_cleared(id, &reason, events);
            }
            AlarmState::Active | AlarmState::ClearedAwaitingAck => {}
        }
    }

    fn absence(&mut self, rule: &Rule, crew: CrewMemberId, now: i64, events: &mut Vec<Event>) {
        let Condition::Absence(a) = &rule.condition else {
            return;
        };
        let (gate_param, gate_min) = a.gate;
        let stale = secs(gate_param.spec().stale_after_s);
        let gate_valid = self
            .gates
            .get(&(crew, gate_param))
            .is_some_and(|(v, t)| *v >= gate_min && now - *t <= stale && now >= *t);
        let skey = (crew, rule.id.clone());
        let st = self.state.entry(skey.clone()).or_default();
        if !gate_valid {
            st.gate_open_since = None;
            return;
        }
        let since = *st.gate_open_since.get_or_insert(now);
        let window = secs(rule.fire_for_s);
        let absent_since = st.last_event.map_or(since, |e| e.max(since));
        let absent_for = now - absent_since;
        if let Some(id) = st.open {
            if absent_for >= window {
                st.events_since = None;
                // Absent again before acknowledgement: the condition is present again.
                if let Some(o) = self.open.get_mut(&id) {
                    if o.alarm.state == AlarmState::ClearedAwaitingAck {
                        o.alarm.state = AlarmState::Active;
                        events.push(Event::Reactivated(o.alarm.clone()));
                    }
                }
            } else if st
                .events_since
                .is_some_and(|es| now - es >= secs(rule.clear_for_s))
            {
                let reason = format!("{} detected for {} s", a.event, rule.clear_for_s);
                self.condition_cleared(id, &reason, events);
            }
            return;
        }
        if absent_for >= window {
            st.events_since = None;
            let notes = vec![format!(
                "no {} for {:.1} s while {} ≥ {}",
                a.event,
                absent_for as f64 / NS_PER_S as f64,
                gate_param.spec().key,
                gate_min
            )];
            let why = self.why(rule, None, None, Some(absent_for), Vec::new(), notes);
            let opened =
                self.open_alarm(rule, crew, AlarmState::Active, None, why, now, None, events);
            if let Some(st) = self.state.get_mut(&skey) {
                st.open = opened;
            }
        }
    }

    fn baseline(&mut self, input: &Input, b: BaselineStats, events: &mut Vec<Event>) {
        let cfg = self.ruleset.baseline.clone();
        let Some(param) = cfg
            .parameters
            .iter()
            .find(|p| p.parameter == input.parameter)
        else {
            return;
        };
        if !(b.mad.is_finite() && b.mad > 0.0 && b.median.is_finite()) {
            return;
        }
        let mut notes = Vec::new();
        if param.at_rest {
            match self.activity_at(input.crew, input.at.0) {
                Some(a) if a > cfg.at_rest.1 => return,
                Some(_) => {}
                None => notes.push("activity unknown: evaluated as if at rest (SR-018)".to_owned()),
            }
        }
        let z = (input.value - b.median) / (1.4826 * b.mad);
        let key = input.parameter.spec().key;
        let rule = Rule {
            id: format!("baseline.{key}"),
            parameter: input.parameter,
            category: param.category,
            priority: Priority::Observe,
            title: format!(
                "{} outside your personal normal",
                input.parameter.spec().label
            ),
            action: "Review in the app; repeat a resting check later.".to_owned(),
            fire_for_s: cfg.for_s,
            clear_for_s: cfg.for_s,
            max_gap_s: cfg.max_gap_s,
            // Same group as the parameter's absolute rules: those supersede the notice; the
            // observe-level notice can never supersede them (SR-002).
            group: key.to_owned(),
            condition: Condition::Baseline,
        };
        notes.push(format!(
            "robust z {z:+.1} against median {} (MAD {}, n = {})",
            b.median, b.mad, b.n
        ));
        let deviating = z.abs() >= cfg.z_fire;
        let skey = (input.crew, rule.id.clone());
        match input.mode {
            MeasurementMode::Continuous => {
                let gap = secs(cfg.max_gap_s);
                let st = self.state.entry(skey.clone()).or_default();
                if deviating {
                    st.clearing = None;
                    let start = advance(&mut st.pending, input.at.0, gap);
                    if st.open.is_none() && input.at.0 - start >= secs(cfg.for_s) {
                        let held = input.at.0 - start;
                        let why = self.why(
                            &rule,
                            None,
                            None,
                            Some(held),
                            vec![Self::reading(input)],
                            notes,
                        );
                        let opened = self.open_alarm(
                            &rule,
                            input.crew,
                            AlarmState::Active,
                            Some(input.value),
                            why,
                            input.at.0,
                            None,
                            events,
                        );
                        if let Some(st) = self.state.get_mut(&skey) {
                            st.open = opened;
                        }
                    }
                } else {
                    st.pending = None;
                    if let (Some(id), true) = (st.open, z.abs() <= cfg.z_clear) {
                        let start = advance(&mut st.clearing, input.at.0, gap);
                        if input.at.0 - start >= secs(cfg.for_s) {
                            st.clearing = None;
                            self.condition_cleared(id, "back within personal normal", events);
                        }
                    } else {
                        st.clearing = None;
                    }
                }
            }
            MeasurementMode::Spot => {
                let n = cfg.spot_n as usize;
                let st = self.state.entry(skey.clone()).or_default();
                st.spot_window.push_back(deviating);
                while st.spot_window.len() > n {
                    st.spot_window.pop_front();
                }
                let k = st.spot_window.iter().filter(|d| **d).count();
                let open = st.open;
                if open.is_none() && k >= cfg.spot_k as usize {
                    notes.push(format!(
                        "{k} of the last {} spot readings deviate",
                        st.spot_window.len()
                    ));
                    let why = self.why(&rule, None, None, None, vec![Self::reading(input)], notes);
                    let opened = self.open_alarm(
                        &rule,
                        input.crew,
                        AlarmState::Active,
                        Some(input.value),
                        why,
                        input.at.0,
                        None,
                        events,
                    );
                    if let Some(st) = self.state.get_mut(&skey) {
                        st.open = opened;
                    }
                } else if let (Some(id), true) = (open, k == 0) {
                    self.condition_cleared(id, "back within personal normal", events);
                }
            }
        }
    }
}
