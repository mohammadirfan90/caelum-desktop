//! Fail-over state machine with a deterministic clock (F-SYS-04, R-OPS-02).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use caelum_cluster::failover::{Action, Failover, InstanceRole};
use caelum_cluster::fence::{Fence, NodeId};
use caelum_cluster::timing::{Timing, TAKEOVER_LIMIT};

fn node(id: &str) -> NodeId {
    NodeId::new(id).unwrap()
}

/// Two nodes on a simulated LAN, stepped in 100 ms ticks (the default tick).
struct Sim {
    nodes: [Failover; 2],
    alive: [bool; 2],
    link_up: bool,
    /// (node index, action, time ms)
    log: Vec<(usize, Action, u64)>,
}

impl Sim {
    fn new(b_start_ms: u64) -> Self {
        Self {
            nodes: [
                Failover::new(node("a"), Timing::default(), None, 0),
                Failover::new(node("b"), Timing::default(), None, b_start_ms),
            ],
            alive: [true, true],
            link_up: true,
            log: Vec::new(),
        }
    }

    fn run(&mut self, from: usize, actions: Vec<Action>, now: u64) {
        for action in actions {
            self.log.push((from, action.clone(), now));
            match action {
                // Persistence succeeds at once in the simulation.
                Action::PersistFence(f) => {
                    let more = self.nodes[from].fence_persisted(&f, now);
                    self.run(from, more, now);
                }
                Action::SendHeartbeat(f) => {
                    let to = 1 - from;
                    if self.link_up && self.alive[to] {
                        match self.nodes[to].on_heartbeat(&f, now) {
                            Ok(more) => self.run(to, more, now),
                            // The receiver answers a stale primary with the current fence.
                            Err(stale) => {
                                let more =
                                    self.nodes[from].on_peer_fence(&stale.current, now).unwrap();
                                self.run(from, more, now);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn step_to(&mut self, until: u64, now: &mut u64) {
        while *now < until {
            *now += 100;
            for i in 0..2 {
                if self.alive[i] {
                    let actions = self.nodes[i].tick(*now);
                    self.run(i, actions, *now);
                }
            }
        }
    }

    fn first(
        &self,
        who: usize,
        after: u64,
        pred: impl Fn(&Action) -> bool,
    ) -> Option<(Action, u64)> {
        self.log
            .iter()
            .find(|(i, a, t)| *i == who && *t >= after && pred(a))
            .map(|(_, a, t)| (a.clone(), *t))
    }
}

#[test]
fn f_sys_04_r_ops_02_standby_takes_over_within_10_s_for_every_failure_phase() {
    // Fail the primary at every 100 ms phase of its heartbeat period.
    for phase in (0..1_000).step_by(100) {
        let mut sim = Sim::new(2_000);
        let mut now = 0;
        sim.step_to(20_000, &mut now);
        assert_eq!(sim.nodes[0].role(), InstanceRole::Primary);
        assert_eq!(sim.nodes[1].role(), InstanceRole::Standby);
        assert_eq!(
            sim.nodes[1].current_primary(),
            Some(&Fence::new(1, node("a")))
        );

        let fail_at = 20_000 + phase;
        sim.step_to(fail_at, &mut now);
        sim.alive[0] = false;
        sim.step_to(fail_at + 12_000, &mut now);

        let (promoted, at) = sim
            .first(1, fail_at, |a| matches!(a, Action::BecomePrimary { .. }))
            .expect("standby promoted");
        let Action::BecomePrimary { fence, .. } = promoted else {
            unreachable!()
        };
        assert_eq!(fence, Fence::new(2, node("b")));
        let detection = at - fail_at;
        assert!(
            detection <= 5_100,
            "phase {phase}: promoted after {detection} ms"
        );
        let (reconnect, _) = sim
            .first(1, fail_at, |a| matches!(a, Action::ReconnectDevices { .. }))
            .expect("device hand-off signalled");
        let Action::ReconnectDevices { deadline_ms, .. } = reconnect else {
            unreachable!()
        };
        let total = deadline_ms - fail_at;
        assert!(
            u128::from(total) <= TAKEOVER_LIMIT.as_millis(),
            "phase {phase}: takeover incl. device reconnection {total} ms"
        );
        assert!(sim.nodes[1].may_act());
    }
}

#[test]
fn f_sys_04_standby_does_not_promote_while_heartbeats_arrive() {
    let mut sim = Sim::new(2_000);
    let mut now = 0;
    sim.step_to(120_000, &mut now);
    assert_eq!(sim.nodes[0].role(), InstanceRole::Primary);
    assert_eq!(sim.nodes[1].role(), InstanceRole::Standby);
    assert!(sim
        .first(1, 0, |a| matches!(a, Action::BecomePrimary { .. }))
        .is_none());
}

#[test]
fn f_sys_04_returning_stale_primary_is_fenced_and_demotes() {
    let mut sim = Sim::new(2_000);
    let mut now = 0;
    sim.step_to(20_000, &mut now);
    // Partition: the standby hears nothing and takes over; the old primary still believes it is
    // primary (it cannot know).
    sim.link_up = false;
    sim.step_to(30_000, &mut now);
    assert!(sim.nodes[1].may_act());
    assert_eq!(sim.nodes[1].acting_fence(), Some(&Fence::new(2, node("b"))));
    // Heal: the first message either way fences the old primary.
    sim.link_up = true;
    sim.step_to(31_000, &mut now);
    let (demote, _) = sim
        .first(0, 30_000, |a| matches!(a, Action::Demote { .. }))
        .expect("stale primary demoted");
    assert_eq!(
        demote,
        Action::Demote {
            from: Fence::new(1, node("a")),
            to: Fence::new(2, node("b")),
        }
    );
    assert!(!sim.nodes[0].may_act(), "a fenced primary never acts");
    assert_eq!(sim.nodes[0].role(), InstanceRole::Standby);
    assert_eq!(
        sim.nodes[0].current_primary(),
        Some(&Fence::new(2, node("b")))
    );
    assert!(sim.nodes[1].may_act());
    assert!(sim
        .first(1, 30_000, |a| matches!(a, Action::Demote { .. }))
        .is_none());

    // A stale fence never counts as liveness: messages under (1, a) are refused.
    let stale = sim.nodes[1].on_heartbeat(&Fence::new(1, node("a")), now);
    assert_eq!(stale.unwrap_err().current, Fence::new(2, node("b")));
}

#[test]
fn f_sys_04_fenced_node_takes_over_again_with_a_new_epoch() {
    let mut sim = Sim::new(2_000);
    let mut now = 0;
    sim.step_to(20_000, &mut now);
    sim.alive[0] = false;
    sim.step_to(30_000, &mut now);
    assert_eq!(sim.nodes[1].acting_fence(), Some(&Fence::new(2, node("b"))));
    // a restarts with its persisted fence: it listens first and follows b.
    sim.nodes[0] = Failover::new(
        node("a"),
        Timing::default(),
        Some(Fence::new(1, node("a"))),
        now,
    );
    sim.alive[0] = true;
    sim.step_to(40_000, &mut now);
    assert_eq!(sim.nodes[0].role(), InstanceRole::Standby);
    assert_eq!(
        sim.nodes[0].current_primary(),
        Some(&Fence::new(2, node("b")))
    );
    // b fails: a takes over under epoch 3, never reusing an epoch.
    sim.alive[1] = false;
    sim.step_to(50_000, &mut now);
    assert_eq!(sim.nodes[0].acting_fence(), Some(&Fence::new(3, node("a"))));
}

#[test]
fn f_sys_04_restarted_node_never_reuses_a_persisted_epoch() {
    let mut f = Failover::new(
        node("a"),
        Timing::default(),
        Some(Fence::new(7, node("b"))),
        0,
    );
    assert_eq!(
        f.tick(5_000),
        vec![Action::PersistFence(Fence::new(8, node("a")))]
    );
}

#[test]
fn f_sys_04_simultaneous_promotion_is_resolved_by_the_fence_order() {
    let mut a = Failover::new(node("a"), Timing::default(), None, 0);
    let mut b = Failover::new(node("b"), Timing::default(), None, 0);
    for (f, id) in [(&mut a, "a"), (&mut b, "b")] {
        f.tick(5_000);
        f.fence_persisted(&Fence::new(1, node(id)), 5_000);
        assert!(f.may_act());
    }
    // (1, b) > (1, a): a demotes, b refuses a.
    let actions = a.on_heartbeat(&Fence::new(1, node("b")), 5_100).unwrap();
    assert!(actions.contains(&Action::Demote {
        from: Fence::new(1, node("a")),
        to: Fence::new(1, node("b")),
    }));
    assert!(b.on_heartbeat(&Fence::new(1, node("a")), 5_100).is_err());
    assert!(!a.may_act());
    assert!(b.may_act());
}

#[test]
fn f_sys_04_pending_promotion_yields_to_a_newer_primary_without_acting() {
    let mut b = Failover::new(
        node("b"),
        Timing::default(),
        Some(Fence::new(1, node("a"))),
        0,
    );
    assert_eq!(
        b.tick(5_000),
        vec![Action::PersistFence(Fence::new(2, node("b")))]
    );
    // Before the fence is durable, a newer primary appears.
    let actions = b.on_heartbeat(&Fence::new(3, node("c")), 5_050).unwrap();
    assert_eq!(
        actions,
        vec![Action::PersistFence(Fence::new(3, node("c")))]
    );
    assert_eq!(b.role(), InstanceRole::Standby);
    assert!(b
        .fence_persisted(&Fence::new(2, node("b")), 5_060)
        .is_empty());
    assert!(!b.may_act());
}

#[test]
fn f_sys_04_own_leftover_fence_is_not_liveness() {
    // A message naming this node's own old fence (e.g. echoed by the peer) records the epoch but
    // never keeps the node from promoting.
    let mut a = Failover::new(node("a"), Timing::default(), None, 0);
    let actions = a.on_peer_fence(&Fence::new(4, node("a")), 1_000).unwrap();
    assert_eq!(
        actions,
        vec![Action::PersistFence(Fence::new(4, node("a")))]
    );
    assert_eq!(a.current_primary(), None);
    assert_eq!(
        a.tick(5_000),
        vec![Action::PersistFence(Fence::new(5, node("a")))]
    );
}
