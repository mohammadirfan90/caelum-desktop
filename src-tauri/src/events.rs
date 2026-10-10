//! The alarm event bus to the UI (M4, docs/03 §3 step 6): what the station's tick and actions
//! produce is pushed to every window as typed events (tauri-specta).
//!
//! Alarms never depend on sign-in (docs/06, ROLE_MATRIX §4), so these events go to every window
//! and carry only what any crew station may show without sign-in: crew member, priority, category,
//! state, title, and sound or escalation stage. Values and the why (physiological detail,
//! ROLE_MATRIX §2) are not in events; they come from authorised commands.

use caelum_alarms::engine::{Alarm, Event};
use caelum_alarms::escalation::{Notice, SoundOffReason, SoundPattern, SoundScope};
use caelum_alarms::ruleset::Audience;
use caelum_core::alarms::Output;
use serde::Serialize;
use specta::Type;

/// One alarm-bus event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type, tauri_specta::Event)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AlarmBusEvent {
    /// An alarm appeared or changed state.
    #[serde(rename_all = "camelCase")]
    Alarm {
        /// Alarm ID.
        alarm: String,
        /// Crew member.
        crew_id: String,
        /// `critical` / `warning` / `observe`.
        priority: String,
        /// Category.
        category: String,
        /// `confirming` / `active` / `cleared_awaiting_ack`.
        state: String,
        /// Short title.
        title: String,
    },
    /// An alarm ended.
    #[serde(rename_all = "camelCase")]
    Ended {
        /// Alarm ID.
        alarm: String,
        /// `resolved` / `superseded` / `not_confirmed`.
        how: String,
    },
    /// An escalation stage was delivered.
    #[serde(rename_all = "camelCase")]
    Stage {
        /// Alarm ID.
        alarm: String,
        /// Stage index.
        stage: u32,
        /// Audience.
        audience: String,
    },
    /// Start or widen the alarm sound.
    #[serde(rename_all = "camelCase")]
    Sound {
        /// Alarm ID.
        alarm: String,
        /// `high` / `medium` / `low`.
        pattern: String,
        /// `notified` (stations of those notified) or `allCrewStations`.
        scope: String,
        /// The wristband alert is sent in addition.
        wristband: bool,
    },
    /// Stop the sound (the visual alarm stays).
    #[serde(rename_all = "camelCase")]
    SoundOff {
        /// Alarm ID.
        alarm: String,
        /// `acknowledged` / `silenced` / `ended`.
        reason: String,
    },
}

fn alarm_event(a: &Alarm, state: &str) -> AlarmBusEvent {
    AlarmBusEvent::Alarm {
        alarm: a.id.to_string(),
        crew_id: a.crew.to_string(),
        priority: a.priority.to_owned(),
        category: a.category.to_owned(),
        state: state.to_owned(),
        title: a.title.clone(),
    }
}

fn audience(a: Audience) -> &'static str {
    match a {
        Audience::Astronaut => "astronaut",
        Audience::Cmo => "cmo",
        Audience::Sound => "sound",
        Audience::Crew => "crew",
        Audience::Ground => "ground",
        Audience::Reminder => "reminder",
    }
}

/// The UI events for one tick or action. OPS records go to path B (M10), not to the UI.
pub fn bus_events(out: &Output) -> Vec<AlarmBusEvent> {
    let mut v = Vec::new();
    for e in &out.events {
        match e {
            Event::Confirming(a) => v.push(alarm_event(a, "confirming")),
            Event::Raised(a) | Event::Reactivated(a) => v.push(alarm_event(a, "active")),
            Event::ClearedAwaitingAck(a) => v.push(alarm_event(a, "cleared_awaiting_ack")),
            Event::NotConfirmed(a) => v.push(AlarmBusEvent::Ended {
                alarm: a.id.to_string(),
                how: "not_confirmed".to_owned(),
            }),
            Event::Resolved { alarm, .. } => v.push(AlarmBusEvent::Ended {
                alarm: alarm.to_string(),
                how: "resolved".to_owned(),
            }),
            Event::Superseded { alarm, .. } => v.push(AlarmBusEvent::Ended {
                alarm: alarm.to_string(),
                how: "superseded".to_owned(),
            }),
            Event::SignalUnusable { .. } | Event::SignalRecovered { .. } => {}
        }
    }
    for n in &out.notices {
        match n {
            Notice::Stage {
                alarm,
                stage,
                audience: a,
                ..
            } => v.push(AlarmBusEvent::Stage {
                alarm: alarm.to_string(),
                stage: u32::try_from(*stage).unwrap_or(u32::MAX),
                audience: audience(*a).to_owned(),
            }),
            Notice::SoundOn {
                alarm,
                pattern,
                scope,
                wristband,
            } => v.push(AlarmBusEvent::Sound {
                alarm: alarm.to_string(),
                pattern: match pattern {
                    SoundPattern::High => "high",
                    SoundPattern::Medium => "medium",
                    SoundPattern::Low => "low",
                }
                .to_owned(),
                scope: match scope {
                    SoundScope::Notified => "notified",
                    SoundScope::AllCrewStations => "allCrewStations",
                }
                .to_owned(),
                wristband: *wristband,
            }),
            Notice::SoundOff { alarm, reason } => v.push(AlarmBusEvent::SoundOff {
                alarm: alarm.to_string(),
                reason: match reason {
                    SoundOffReason::Acknowledged => "acknowledged",
                    SoundOffReason::SilencedUntil(_) => "silenced",
                    SoundOffReason::Ended => "ended",
                }
                .to_owned(),
            }),
            Notice::Ops(_) => {}
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use caelum_alarms::engine::{AlarmState, Why};
    use caelum_domain::ids::{AlarmId, CrewMemberId};

    use super::*;

    /// ROLE_MATRIX §2: events reach every window, so they never carry values or the why.
    #[test]
    fn bus_events_carry_no_physiological_detail() {
        let alarm = Alarm {
            id: AlarmId::from_uuid(uuid::Uuid::from_u128(1)),
            crew: CrewMemberId::from_uuid(uuid::Uuid::from_u128(2)),
            rule_id: "spo2.critical".to_owned(),
            priority: "critical",
            category: "RESPIRATORY",
            state: AlarmState::Active,
            latched: true,
            trigger_value: Some(80.0),
            title: "Severe low blood oxygen".to_owned(),
            action: "act".to_owned(),
            why: Why {
                rule_id: "spo2.critical".to_owned(),
                ruleset_version: "v".to_owned(),
                parameter: "spo2",
                unit: "%",
                threshold: Some(85.0),
                op: Some("<"),
                held_s: Some(15.0),
                readings: Vec::new(),
                notes: vec!["secret note".to_owned()],
            },
            created_at: 0,
        };
        let out = Output {
            events: vec![Event::Raised(alarm.clone())],
            notices: vec![Notice::SoundOn {
                alarm: alarm.id,
                pattern: SoundPattern::High,
                scope: SoundScope::AllCrewStations,
                wristband: true,
            }],
        };
        let events = bus_events(&out);
        assert_eq!(events.len(), 2);
        let json = serde_json::to_string(&events).unwrap();
        for leak in ["80", "spo2.critical", "\"%\"", "secret note", "85"] {
            assert!(!json.contains(leak), "{leak} leaked: {json}");
        }
        assert!(json.contains("\"type\":\"alarm\""));
        assert!(json.contains("\"scope\":\"allCrewStations\""));
    }
}
