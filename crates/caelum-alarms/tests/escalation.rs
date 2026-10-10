//! M4 gate: the escalation timeline matches docs/06 exactly; a restart mid-escalation resumes at
//! the correct stage; silence expires and re-sounds; crew stations sound without any wristband
//! (F-ALM-03 priorities and their sound patterns, F-ALM-04, F-ALM-05; SR-007, SR-008, SR-015;
//! docs/06 §3a OPS fields).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

use caelum_alarms::escalation::{
    EscalationError, Escalator, Notice, OpsEscalation, SoundOffReason, SoundPattern, SoundScope,
    Tracked,
};
use caelum_alarms::ruleset::{Audience, Priority, Ruleset};
use caelum_alarms::signing::{built_in_keys, verify, TrustPolicy};
use caelum_alarms::{BUNDLED_RULESET, BUNDLED_SIGNATURES};
use caelum_domain::ids::{AlarmId, CrewMemberId};
use caelum_domain::time::TaiNs;

const S: i64 = 1_000_000_000;
const T0: i64 = 2_000_000_000 * S;
const SAFETY_DOC: &str = include_str!("../../../docs/06_SAFETY_AND_ALARMS.md");

fn ruleset() -> Ruleset {
    verify(
        BUNDLED_RULESET.as_bytes(),
        BUNDLED_SIGNATURES,
        &built_in_keys(),
        TrustPolicy::Development,
    )
    .unwrap()
    .ruleset
}

fn alarm() -> AlarmId {
    AlarmId::from_uuid(uuid::Uuid::from_u128(5))
}

fn tracked(priority: Priority) -> Tracked {
    Tracked {
        crew: CrewMemberId::from_uuid(uuid::Uuid::from_u128(1)),
        priority,
        category: "RESPIRATORY",
        state: "active",
        created_at: T0,
        next_stage: 0,
        acknowledged: false,
        silenced_until: None,
    }
}

fn at(s: i64) -> TaiNs {
    TaiNs(T0 + s * S)
}

/// docs/06 §3 "Escalation (...)": the plan per priority as (audience, seconds).
fn doc_plans() -> Vec<(Priority, Vec<(Audience, u32)>)> {
    let start = SAFETY_DOC
        .find("Escalation (seconds after creation")
        .unwrap();
    let end = SAFETY_DOC[start..].find("\n\n").unwrap() + start;
    let text = SAFETY_DOC[start..end].replace('\n', " ");
    // Remove parenthetical remarks, then split per priority.
    let mut plain = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (name, priority) in [
        ("critical:", Priority::Critical),
        ("Warning:", Priority::Warning),
        ("Observe:", Priority::Observe),
    ] {
        let from = plain.find(name).unwrap() + name.len();
        let rest = &plain[from..];
        let stop = rest.find('.').unwrap();
        let stages = rest[..stop]
            .split(',')
            .map(|p| {
                let mut w = p.split_whitespace();
                let who = w.next().unwrap().to_lowercase();
                let s: u32 = w.next().unwrap().parse().unwrap();
                let audience = match who.as_str() {
                    "astronaut" => Audience::Astronaut,
                    "cmo" => Audience::Cmo,
                    "sound" => Audience::Sound,
                    "crew" => Audience::Crew,
                    "ground" => Audience::Ground,
                    "reminder" => Audience::Reminder,
                    other => panic!("unknown audience {other}"),
                };
                (audience, s)
            })
            .collect();
        out.push((priority, stages));
    }
    out
}

fn record(delivered: &mut Vec<(Audience, u32)>, t: i64, notices: Vec<Notice>) {
    for n in notices {
        if let Notice::Stage { audience, .. } = n {
            delivered.push((audience, u32::try_from(t).unwrap()));
        }
    }
}

/// M4 gate: the signed plans equal docs/06, and stages are delivered exactly at their times.
/// R-ALM-01: a critical alarm's first stage (astronaut and CMO) is delivered at 0 s, in the call
/// that raises it.
#[test]
fn escalation_timeline_matches_docs_06_exactly() {
    let rs = ruleset();
    for (priority, expected) in doc_plans() {
        let plan: Vec<(Audience, u32)> = rs
            .escalation
            .plan(priority)
            .iter()
            .map(|s| (s.audience, s.after_s))
            .collect();
        assert_eq!(plan, expected, "{priority:?}");
        // Simulate: tick every second (critical, warning) or at each stage time (observe).
        let mut e = Escalator::new(rs.escalation.clone());
        let mut delivered = Vec::new();
        record(
            &mut delivered,
            0,
            e.track(alarm(), tracked(priority), at(0)),
        );
        let last = i64::from(expected.last().unwrap().1);
        let times: Vec<i64> = if last > 600 {
            expected.iter().map(|(_, s)| i64::from(*s)).collect()
        } else {
            (1..=last + 5).collect()
        };
        for t in times {
            let before = delivered.len();
            record(&mut delivered, t, e.tick(at(t)));
            // Never early: everything delivered at t is due at t.
            for (_, due) in &delivered[before..] {
                assert!(i64::from(*due) <= t);
            }
        }
        assert_eq!(
            delivered, expected,
            "{priority:?}: delivered at the documented times"
        );
    }
}

/// docs/06 §3a: the ground stage sends exactly the allowed OPS fields.
#[test]
fn the_ground_stage_sends_only_the_ops_fields() {
    let mut e = Escalator::new(ruleset().escalation);
    e.track(alarm(), tracked(Priority::Critical), at(0));
    let ops: Vec<OpsEscalation> = e
        .tick(at(60))
        .into_iter()
        .filter_map(|n| {
            if let Notice::Ops(o) = n {
                Some(o)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(ops.len(), 1);
    let json = serde_json::to_value(ops[0]).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "at_tai",
            "category",
            "created_at_tai",
            "crew_id",
            "priority",
            "stage",
            "state"
        ]
    );
    assert_eq!(
        (ops[0].priority, ops[0].stage, ops[0].at_tai),
        ("critical", 4, T0 + 60 * S)
    );
}

/// M4 gate, SR-007: a restart mid-escalation resumes at the stored stage: nothing repeats, the
/// remaining stages come on time, and stages that fell due while down are delivered at once.
#[test]
fn sr_007_restart_mid_escalation_resumes_at_the_correct_stage() {
    let plans = ruleset().escalation;
    let mut before = Escalator::new(plans.clone());
    let mut next = 0;
    for t in [0, 15, 20] {
        let notices = if t == 0 {
            before.track(alarm(), tracked(Priority::Critical), at(0))
        } else {
            before.tick(at(t))
        };
        for n in notices {
            if let Notice::Stage { next_stage, .. } = n {
                next = next_stage; // what the caller persists
            }
        }
    }
    assert_eq!(
        next, 3,
        "astronaut, CMO and sound delivered before the restart"
    );
    // Restart: a new escalator restores the stored stage.
    let mut after = Escalator::new(plans.clone());
    let sound = after.restore(
        alarm(),
        Tracked {
            next_stage: next,
            ..tracked(Priority::Critical)
        },
    );
    assert!(
        matches!(
            &sound[..],
            [Notice::SoundOn {
                scope: SoundScope::AllCrewStations,
                wristband: true,
                ..
            }]
        ),
        "sound resumes: {sound:?}"
    );
    assert!(after.tick(at(25)).is_empty(), "no stage repeats");
    let crew = after.tick(at(30));
    assert!(
        matches!(
            crew.first(),
            Some(Notice::Stage {
                stage: 3,
                audience: Audience::Crew,
                ..
            })
        ),
        "{crew:?}"
    );
    assert!(after
        .tick(at(60))
        .iter()
        .any(|n| matches!(n, Notice::Ops(_))));
    // Down for longer: everything due is delivered on the first tick after the restart.
    let mut late = Escalator::new(plans);
    late.restore(
        alarm(),
        Tracked {
            next_stage: 1,
            ..tracked(Priority::Critical)
        },
    );
    let stages: Vec<Audience> = late
        .tick(at(100))
        .into_iter()
        .filter_map(|n| {
            if let Notice::Stage { audience, .. } = n {
                Some(audience)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        stages,
        [
            Audience::Cmo,
            Audience::Sound,
            Audience::Crew,
            Audience::Ground
        ]
    );
}

/// M4 gate, SR-008, R-ALM-05: silence is clamped to 2 min (critical), never stops escalation, and the alarm
/// sounds again when it expires.
#[test]
fn sr_008_silence_expires_and_resounds_while_escalation_continues() {
    let mut e = Escalator::new(ruleset().escalation);
    e.track(alarm(), tracked(Priority::Critical), at(0));
    e.tick(at(15));
    let (notices, applied) = e.silence(alarm(), 600, at(20)).unwrap();
    assert_eq!(applied, 120, "clamped to the SR-008 maximum");
    assert_eq!(
        notices,
        [Notice::SoundOff {
            alarm: alarm(),
            reason: SoundOffReason::SilencedUntil(T0 + 140 * S)
        }]
    );
    let during = e.tick(at(30));
    assert!(
        during.iter().any(|n| matches!(
            n,
            Notice::Stage {
                audience: Audience::Crew,
                ..
            }
        )),
        "escalation continues"
    );
    assert!(
        !during.iter().any(|n| matches!(n, Notice::SoundOn { .. })),
        "no sound while silenced"
    );
    assert!(!e
        .tick(at(139))
        .iter()
        .any(|n| matches!(n, Notice::SoundOn { .. })));
    let expired = e.tick(at(140));
    assert!(
        expired.iter().any(|n| matches!(
            n,
            Notice::SoundOn {
                pattern: SoundPattern::High,
                scope: SoundScope::AllCrewStations,
                ..
            }
        )),
        "re-sounds: {expired:?}"
    );
}

/// SR-008, R-ALM-05: silencing is time-limited per priority.
#[test]
fn sr_008_warning_silence_is_at_most_ten_minutes_and_observe_cannot_be_silenced() {
    let mut e = Escalator::new(ruleset().escalation);
    e.track(alarm(), tracked(Priority::Warning), at(0));
    assert_eq!(e.silence(alarm(), 3_600, at(1)).unwrap().1, 600);
    assert_eq!(
        e.silence(alarm(), 0, at(1)),
        Err(EscalationError::ZeroSilence)
    );
    let observe = AlarmId::from_uuid(uuid::Uuid::from_u128(6));
    e.track(observe, tracked(Priority::Observe), at(0));
    assert_eq!(
        e.silence(observe, 60, at(1)),
        Err(EscalationError::NotSilenceable)
    );
    let ghost = AlarmId::from_uuid(uuid::Uuid::from_u128(99));
    assert_eq!(
        e.silence(ghost, 60, at(1)),
        Err(EscalationError::UnknownAlarm)
    );
    assert_eq!(e.acknowledge(ghost), Err(EscalationError::UnknownAlarm));
    assert_eq!(
        e.escalate_now(ghost, at(1)),
        Err(EscalationError::UnknownAlarm)
    );
}

/// M4 gate, SR-015: crew stations sound by themselves; the wristband alert is only "in addition".
#[test]
fn sr_015_crew_stations_sound_with_or_without_a_wristband() {
    let mut e = Escalator::new(ruleset().escalation);
    let start = e.track(alarm(), tracked(Priority::Critical), at(0));
    assert!(
        start.iter().any(|n| matches!(
            n,
            Notice::SoundOn {
                scope: SoundScope::Notified,
                wristband: false,
                ..
            }
        )),
        "the notified stations sound at once: {start:?}"
    );
    let sound = e.tick(at(15));
    match sound.iter().find(|n| matches!(n, Notice::SoundOn { .. })) {
        Some(Notice::SoundOn {
            scope, wristband, ..
        }) => {
            assert_eq!(
                *scope,
                SoundScope::AllCrewStations,
                "every crew station sounds"
            );
            assert!(*wristband, "the wristband alert is sent in addition");
        }
        other => panic!("{other:?}"),
    }
    // Warning: no sound stage; every crew station sounds from the crew stage.
    let mut w = Escalator::new(ruleset().escalation);
    let wid = AlarmId::from_uuid(uuid::Uuid::from_u128(7));
    w.track(wid, tracked(Priority::Warning), at(0));
    let crew = w.tick(at(60));
    assert!(
        crew.iter().any(|n| matches!(
            n,
            Notice::SoundOn {
                scope: SoundScope::AllCrewStations,
                wristband: false,
                pattern: SoundPattern::Medium,
                ..
            }
        )),
        "{crew:?}"
    );
}

/// F-ALM-05: acknowledgement stops escalation and sound; manual escalation delivers the next stage;
/// an ended alarm stops sounding.
#[test]
fn acknowledge_stops_escalation_and_manual_escalation_advances() {
    let mut e = Escalator::new(ruleset().escalation);
    e.track(alarm(), tracked(Priority::Critical), at(0));
    let manual = e.escalate_now(alarm(), at(3)).unwrap();
    assert!(
        matches!(
            manual.first(),
            Some(Notice::Stage {
                audience: Audience::Sound,
                ..
            })
        ),
        "{manual:?}"
    );
    assert_eq!(
        e.acknowledge(alarm()).unwrap(),
        [Notice::SoundOff {
            alarm: alarm(),
            reason: SoundOffReason::Acknowledged
        }]
    );
    assert!(e.tick(at(600)).is_empty(), "no stage after acknowledgement");
    assert!(e.tracked(alarm()).unwrap().acknowledged);
    e.set_state(alarm(), "cleared_awaiting_ack");
    assert_eq!(
        e.ended(alarm()),
        [Notice::SoundOff {
            alarm: alarm(),
            reason: SoundOffReason::Ended
        }]
    );
    assert!(e.ended(alarm()).is_empty());
    // A fully escalated alarm: manual escalation has nothing left.
    let mut f = Escalator::new(ruleset().escalation);
    f.track(alarm(), tracked(Priority::Critical), at(0));
    f.tick(at(60));
    assert!(f.escalate_now(alarm(), at(61)).unwrap().is_empty());
}
