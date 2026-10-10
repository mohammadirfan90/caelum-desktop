//! Outbound routing modes A / A' / B (F-GND-13, F-GND-14, SR-015, R-INT-06, R-INT-07; docs/03 §1,
//! docs/04 §3 and §3a).
//!
//! [`route`] is a pure decision function: the same inputs always give the same routes, and it has
//! no access to any I/O. The caller dispatches each returned [`Route`]:
//!
//! - [`Route::PathB`]: OPS channel, built by `caelum-ground` as telemetry-only CCSDS packets and
//!   sent directly to the vehicle. **Always** chosen for alarm events and ground escalations,
//!   whatever the state of the medical data system (SR-015, R-INT-06); never chosen for anything
//!   else from this module (the OPS channel carries no physiology).
//! - [`Route::PathA`]: the medical data system outbox ([`crate::outbox`]).
//! - [`Route::PathAPrime`]: direct CFDP through the ground downlink queue on the MEDICAL channel
//!   (fallback per mission when the medical data system is not usable).
//! - [`Route::HoldLocal`]: kept in the outbox and synchronised on reconnect.
//! - [`Route::NotSent`]: kept in the local record only (F-GND-14: raw waveforms without an event
//!   link or a ground request).
//!
//! Medical record path: when the medical data system accepts path A (Connected or Degraded), the
//! item goes to path A. Otherwise the mission fallback applies:
//! - `HoldUntilReconnect`: `HoldLocal`.
//! - `DirectCfdp`: `PathAPrime` **and** `HoldLocal` when a system is configured but unreachable,
//!   so the ground record gets the item now and the onboard record is still complete after
//!   reconnect (R-INT-07: no data loss; the ground side de-duplicates by the shared client id);
//!   `PathAPrime` alone when no medical data system is configured (nothing to reconnect to).
//!
//! Alarm events and escalations always also take the medical record path, so the clinical record
//! holds the full alarm detail (MEDICAL channel).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::health::MdsHealth;

/// What is being sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemKind {
    /// An alarm event (raise, acknowledgement, clearance).
    AlarmEvent,
    /// The ground escalation stage of an alarm.
    AlarmGroundEscalation,
    /// A periodic observation summary.
    ObservationSummary,
    /// A file: DICOM study, ECG file or report.
    Attachment,
    /// A raw waveform segment.
    RawWaveform {
        /// Linked to an alarm or clinical event.
        event_linked: bool,
        /// Requested by ground.
        ground_requested: bool,
    },
}

/// What to do with medical-record items when the medical data system is not usable (per mission).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FallbackPolicy {
    /// Keep in the outbox until the medical data system is back.
    HoldUntilReconnect,
    /// Send directly by CFDP via the ground downlink queue (path A').
    DirectCfdp,
}

/// One outbound route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Route {
    /// OPS channel, direct to the vehicle (cFS per ICD-VEH).
    PathB,
    /// Medical data system outbox.
    PathA,
    /// Direct CFDP via the ground downlink queue (MEDICAL channel).
    PathAPrime,
    /// Kept in the outbox until the medical data system reconnects.
    HoldLocal,
    /// Kept in the local record only; not sent anywhere.
    NotSent,
}

/// The set of routes for one item (never empty).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Routes(BTreeSet<Route>);

impl Routes {
    fn of(routes: &[Route]) -> Self {
        Self(routes.iter().copied().collect())
    }

    /// Whether `route` is included.
    pub fn contains(&self, route: Route) -> bool {
        self.0.contains(&route)
    }

    /// The routes, in [`Route`] order.
    pub fn iter(&self) -> impl Iterator<Item = Route> + '_ {
        self.0.iter().copied()
    }

    /// Whether the item belongs in the medical data system outbox (path A or held for it).
    pub fn uses_outbox(&self) -> bool {
        self.contains(Route::PathA) || self.contains(Route::HoldLocal)
    }
}

impl ItemKind {
    /// Whether the item is an alarm event or escalation (always path B, SR-015).
    pub fn is_alarm(self) -> bool {
        matches!(self, Self::AlarmEvent | Self::AlarmGroundEscalation)
    }
}

/// Decides the routes of one item (F-GND-13, F-GND-14, SR-015). Pure and total.
pub fn route(kind: ItemKind, policy: FallbackPolicy, health: MdsHealth) -> Routes {
    if let ItemKind::RawWaveform {
        event_linked: false,
        ground_requested: false,
    } = kind
    {
        return Routes::of(&[Route::NotSent]);
    }
    let medical: &[Route] = match (health.accepts_path_a(), policy, health) {
        (true, _, _) => &[Route::PathA],
        (false, FallbackPolicy::HoldUntilReconnect, _) => &[Route::HoldLocal],
        (false, FallbackPolicy::DirectCfdp, MdsHealth::NotConfigured) => &[Route::PathAPrime],
        (false, FallbackPolicy::DirectCfdp, _) => &[Route::PathAPrime, Route::HoldLocal],
    };
    let mut routes = Routes::of(medical);
    if kind.is_alarm() {
        routes.0.insert(Route::PathB);
    }
    routes
}

#[cfg(test)]
mod tests {
    use super::*;

    use Route::{HoldLocal, NotSent, PathA, PathAPrime, PathB};

    const KINDS: [ItemKind; 7] = [
        ItemKind::AlarmEvent,
        ItemKind::AlarmGroundEscalation,
        ItemKind::ObservationSummary,
        ItemKind::Attachment,
        ItemKind::RawWaveform {
            event_linked: false,
            ground_requested: false,
        },
        ItemKind::RawWaveform {
            event_linked: true,
            ground_requested: false,
        },
        ItemKind::RawWaveform {
            event_linked: false,
            ground_requested: true,
        },
    ];
    const POLICIES: [FallbackPolicy; 2] = [
        FallbackPolicy::HoldUntilReconnect,
        FallbackPolicy::DirectCfdp,
    ];
    const HEALTHS: [MdsHealth; 4] = [
        MdsHealth::Connected,
        MdsHealth::Degraded,
        MdsHealth::Unreachable,
        MdsHealth::NotConfigured,
    ];

    /// The expected medical-record routes, written out independently of the implementation.
    fn expected_medical(policy: FallbackPolicy, health: MdsHealth) -> Vec<Route> {
        use FallbackPolicy::{DirectCfdp, HoldUntilReconnect};
        use MdsHealth::{Connected, Degraded, NotConfigured, Unreachable};
        match (policy, health) {
            (HoldUntilReconnect, Connected) => vec![PathA],
            (HoldUntilReconnect, Degraded) => vec![PathA],
            (HoldUntilReconnect, Unreachable) => vec![HoldLocal],
            (HoldUntilReconnect, NotConfigured) => vec![HoldLocal],
            (DirectCfdp, Connected) => vec![PathA],
            (DirectCfdp, Degraded) => vec![PathA],
            (DirectCfdp, Unreachable) => vec![PathAPrime, HoldLocal],
            (DirectCfdp, NotConfigured) => vec![PathAPrime],
        }
    }

    fn expected(kind: ItemKind, policy: FallbackPolicy, health: MdsHealth) -> Vec<Route> {
        let mut v = match kind {
            ItemKind::RawWaveform {
                event_linked: false,
                ground_requested: false,
            } => vec![NotSent],
            ItemKind::AlarmEvent | ItemKind::AlarmGroundEscalation => {
                let mut v = expected_medical(policy, health);
                v.push(PathB);
                v
            }
            _ => expected_medical(policy, health),
        };
        v.sort();
        v
    }

    /// F-GND-13, F-GND-14, SR-015, R-INT-06/07: exhaustive table over kind × policy × health.
    #[test]
    fn f_gnd_13_sr_015_exhaustive_routing_table() {
        let mut cases = 0;
        for kind in KINDS {
            for policy in POLICIES {
                for health in HEALTHS {
                    let got: Vec<Route> = route(kind, policy, health).iter().collect();
                    assert_eq!(
                        got,
                        expected(kind, policy, health),
                        "{kind:?} {policy:?} {health:?}"
                    );
                    cases += 1;
                }
            }
        }
        assert_eq!(cases, 7 * 2 * 4);
    }

    /// SR-015, R-INT-06: every alarm item goes on path B whatever the medical system's state; path
    /// B carries nothing else; alarms always also reach the medical record path.
    #[test]
    fn sr_015_r_int_06_alarms_always_on_path_b() {
        for kind in KINDS {
            for policy in POLICIES {
                for health in HEALTHS {
                    let r = route(kind, policy, health);
                    assert_eq!(r.contains(PathB), kind.is_alarm(), "{kind:?}");
                    if kind.is_alarm() {
                        assert!(
                            r.contains(PathA) || r.contains(PathAPrime) || r.contains(HoldLocal)
                        );
                    }
                    assert!(r.iter().next().is_some(), "never empty");
                }
            }
        }
    }

    /// F-GND-14: raw waveforms are not sent without an event link or a ground request.
    #[test]
    fn f_gnd_14_raw_waveforms_summary_first() {
        for policy in POLICIES {
            for health in HEALTHS {
                let unlinked = ItemKind::RawWaveform {
                    event_linked: false,
                    ground_requested: false,
                };
                let r = route(unlinked, policy, health);
                assert_eq!(r.iter().collect::<Vec<_>>(), [NotSent]);
                assert!(!r.uses_outbox());
                let both = ItemKind::RawWaveform {
                    event_linked: true,
                    ground_requested: true,
                };
                assert!(!route(both, policy, health).contains(NotSent));
            }
        }
    }

    /// R-INT-07: with the A' policy and an unreachable system, the item goes by CFDP and is also
    /// held for the onboard record, so nothing is lost.
    #[test]
    fn r_int_07_direct_cfdp_also_holds_for_reconnect() {
        let r = route(
            ItemKind::Attachment,
            FallbackPolicy::DirectCfdp,
            MdsHealth::Unreachable,
        );
        assert!(r.contains(PathAPrime) && r.contains(HoldLocal) && r.uses_outbox());
        let healthy = route(
            ItemKind::Attachment,
            FallbackPolicy::DirectCfdp,
            MdsHealth::Connected,
        );
        assert!(healthy.uses_outbox() && !healthy.contains(PathAPrime));
    }
}
