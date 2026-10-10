//! Signed log replication, sans I/O (F-SYS-04, R-OPS-02).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use caelum_cluster::fence::Fence;
use caelum_cluster::log::{
    verify_chain, verify_next, ChainHead, EntrySigner, LogEntry, LogError, TrustedSigners,
};
use caelum_cluster::replication::{
    checkpoints, checkpoints_between, FollowerSession, LeaderSession, LogStore, ReplicationError,
    SyncStep,
};
use caelum_cluster::wire::{PeerMessage, WireEntry};
use common::*;

fn append(log: &mut MemLog, fence: &Fence, signer: &dyn EntrySigner, i: u64) -> LogEntry {
    let entry = LogEntry::create(&log.head().unwrap(), fence.clone(), body(i), signer).unwrap();
    log.append(&entry).unwrap();
    entry
}

/// Checkpoints and probes until the leader knows the exact common point.
fn negotiate(lead: &mut LeaderSession, leader: &MemLog, follower: &MemLog) -> u64 {
    let mut points = checkpoints(follower).unwrap();
    for _ in 0..10 {
        match lead.on_checkpoints(leader, &points).unwrap() {
            SyncStep::Resume(after) => return after,
            SyncStep::Probe { after, before } => {
                points = checkpoints_between(follower, after, before).unwrap();
            }
        }
    }
    panic!("negotiation did not converge");
}

/// Runs one leader→follower session until the follower has everything (or `limit` entries).
fn replicate(
    leader: &MemLog,
    fence: &Fence,
    follower: &mut MemLog,
    limit: Option<usize>,
) -> (u64, Vec<LogEntry>) {
    let mut lead = LeaderSession::new(fence.clone(), 64);
    let mut follow = FollowerSession::new(fence.clone(), follower.head().unwrap());
    let after = negotiate(&mut lead, leader, follower);
    let diverted = follow.on_resume(follower, after).unwrap();
    let mut sent = 0;
    loop {
        let batch = lead.next_batch(leader, 16).unwrap();
        if batch.is_empty() {
            break;
        }
        for entry in &batch {
            if limit.is_some_and(|l| sent >= l) {
                return (lead.acked(), diverted);
            }
            // Through the wire encoding, as on the network.
            let wire = WireEntry::from(entry);
            let decoded = LogEntry::try_from(&wire).unwrap();
            follow.accept(follower, &trust_ab(), &decoded).unwrap();
            sent += 1;
        }
        lead.on_ack(fence, follow.ack_seq()).unwrap();
    }
    (lead.acked(), diverted)
}

#[test]
fn f_sys_04_r_ops_02_no_acknowledged_entry_is_lost_across_takeover() {
    let a_signer = signer("a", 1);
    let b_signer = signer("b", 2);
    let fence_a = Fence::new(1, node("a"));
    let fence_b = Fence::new(2, node("b"));
    let mut a_log = MemLog::default();
    let mut b_log = MemLog::default();

    for i in 0..300 {
        append(&mut a_log, &fence_a, &a_signer, i);
    }
    // Replicate part of the log: 250 entries reach the standby and are acknowledged.
    let (acked, _) = replicate(&a_log, &fence_a, &mut b_log, Some(250));
    assert!((240..=250).contains(&acked), "acked {acked}");
    // The primary keeps writing, then dies: entries after `acked` were never acknowledged.
    for i in 300..320 {
        append(&mut a_log, &fence_a, &a_signer, i);
    }
    let a_before = a_log.entries();

    // b takes over: everything acknowledged is in b's log, identical.
    let b_entries = b_log.entries();
    for seq in 1..=acked {
        assert_eq!(
            b_entries[seq as usize - 1],
            a_before[seq as usize - 1],
            "seq {seq}"
        );
    }
    let fork = b_log.head().unwrap().seq;
    for i in 1000..1010 {
        append(&mut b_log, &fence_b, &b_signer, i);
    }

    // a returns as b's follower: its unacknowledged suffix is diverted, then it catches up.
    let (_, diverted) = replicate(&b_log, &fence_b, &mut a_log, None);
    assert_eq!(diverted.len() as u64, 320 - fork);
    assert_eq!(
        a_log.quarantine(),
        diverted,
        "never deleted: kept for review"
    );
    assert_eq!(a_log.entries(), b_log.entries());
    verify_chain(&ChainHead::genesis(), &a_log.entries(), &trust_ab()).unwrap();
}

#[test]
fn f_sys_04_follower_acknowledges_only_what_it_persisted() {
    let a_signer = signer("a", 1);
    let fence = Fence::new(1, node("a"));
    let mut a_log = MemLog::default();
    let mut b_log = MemLog::default();
    for i in 0..5 {
        append(&mut a_log, &fence, &a_signer, i);
    }
    let mut lead = LeaderSession::new(fence.clone(), 64);
    let mut follow = FollowerSession::new(fence.clone(), ChainHead::genesis());
    let after = negotiate(&mut lead, &a_log, &b_log);
    follow.on_resume(&mut b_log, after).unwrap();
    let batch = lead.next_batch(&a_log, 16).unwrap();
    follow.accept(&mut b_log, &trust_ab(), &batch[0]).unwrap();
    follow.accept(&mut b_log, &trust_ab(), &batch[1]).unwrap();
    b_log.set_failing(true);
    let failed = follow.accept(&mut b_log, &trust_ab(), &batch[2]);
    assert!(matches!(failed, Err(ReplicationError::Store(_))));
    assert_eq!(follow.ack_seq(), 2, "the failed entry is not acknowledged");
    assert_eq!(lead.on_ack(&fence, follow.ack_seq()).unwrap(), 2);
    // An acknowledgement of something never sent, or under another fence, is a protocol error.
    assert!(lead.on_ack(&fence, 99).is_err());
    assert!(lead.on_ack(&Fence::new(9, node("b")), 1).is_err());
}

#[test]
fn f_sys_04_leader_window_limits_unacknowledged_entries() {
    let a_signer = signer("a", 1);
    let fence = Fence::new(1, node("a"));
    let mut a_log = MemLog::default();
    for i in 0..100 {
        append(&mut a_log, &fence, &a_signer, i);
    }
    let mut lead = LeaderSession::new(fence.clone(), 10);
    assert_eq!(
        lead.on_checkpoints(&a_log, &[]).unwrap(),
        SyncStep::Resume(0)
    );
    assert_eq!(lead.next_batch(&a_log, 64).unwrap().len(), 10);
    assert!(lead.next_batch(&a_log, 64).unwrap().is_empty());
    lead.on_ack(&fence, 4).unwrap();
    let batch = lead.next_batch(&a_log, 64).unwrap();
    assert_eq!(batch.first().map(|e| e.seq), Some(11));
    assert_eq!(batch.len(), 4);
}

#[test]
fn f_sys_04_checkpoints_find_the_divergence_point_in_long_logs() {
    let a_signer = signer("a", 1);
    let b_signer = signer("b", 2);
    let fence_a = Fence::new(1, node("a"));
    let fence_b = Fence::new(2, node("b"));
    for fork in [0u64, 1, 500, 1_500, 1_990] {
        let mut a_log = MemLog::default();
        let mut b_log = MemLog::default();
        for i in 0..fork {
            let e = append(&mut a_log, &fence_a, &a_signer, i);
            b_log.append(&e).unwrap();
        }
        for i in fork..2_000 {
            append(&mut a_log, &fence_a, &a_signer, i);
        }
        for i in 0..7 {
            append(&mut b_log, &fence_b, &b_signer, 10_000 + i);
        }
        let points = checkpoints(&a_log).unwrap();
        assert!(points.len() < 120, "checkpoint list stays small");
        let mut lead = LeaderSession::new(fence_b.clone(), 64);
        let after = negotiate(&mut lead, &b_log, &a_log);
        // The exact fork is found: only entries the leader lacks are diverted.
        assert_eq!(after, fork);
        let mut follow = FollowerSession::new(fence_b.clone(), a_log.head().unwrap());
        let diverted = follow.on_resume(&mut a_log, after).unwrap();
        assert_eq!(diverted.len() as u64, 2_000 - after);
        let mut rest = b_log.range(after + 1, usize::MAX).unwrap();
        for e in rest.drain(..) {
            follow.accept(&mut a_log, &trust_ab(), &e).unwrap();
        }
        assert_eq!(a_log.entries(), b_log.entries(), "fork {fork}");
    }
}

#[test]
fn f_sys_04_probe_answers_must_stay_in_range_and_make_progress() {
    let a_signer = signer("a", 1);
    let b_signer = signer("b", 2);
    let fence_a = Fence::new(1, node("a"));
    let mut a_log = MemLog::default();
    let mut b_log = MemLog::default();
    for i in 0..100 {
        let e = append(&mut a_log, &fence_a, &a_signer, i);
        b_log.append(&e).unwrap();
    }
    for i in 100..300 {
        append(&mut a_log, &fence_a, &a_signer, i);
        append(&mut b_log, &Fence::new(2, node("b")), &b_signer, i);
    }
    let first = checkpoints(&b_log).unwrap();
    let mut lead = LeaderSession::new(fence_a.clone(), 64);
    let SyncStep::Probe { after, before } = lead.on_checkpoints(&a_log, &first).unwrap() else {
        panic!("coarse checkpoints need a probe here")
    };
    assert!(after < 100 && before > 101);
    // An empty answer makes no progress; a point outside the range is refused.
    assert!(lead.on_checkpoints(&a_log, &[]).is_err());
    let mut lead = LeaderSession::new(fence_a, 64);
    lead.on_checkpoints(&a_log, &first).unwrap();
    assert!(lead.on_checkpoints(&a_log, &first[..1]).is_err());
    // Sending entries before the resume point is agreed sends nothing.
    assert!(lead.next_batch(&a_log, 16).unwrap().is_empty());
}

fn chain(n: u64) -> Vec<LogEntry> {
    let s = signer("a", 1);
    let mut log = MemLog::default();
    for i in 0..n {
        append(&mut log, &Fence::new(1, node("a")), &s, i);
    }
    log.entries()
}

#[test]
fn f_sys_04_tampered_replication_entries_are_refused() {
    let entries = chain(3);
    let trust = trust_ab();
    let head1 = ChainHead::of(&entries[0]);

    let mut payload = entries[1].clone();
    payload.payload[0] ^= 1;
    assert_eq!(
        verify_next(&head1, &payload, &trust),
        Err(LogError::BadSignature(2))
    );

    let mut sig = entries[1].clone();
    sig.signature[5] ^= 1;
    assert_eq!(
        verify_next(&head1, &sig, &trust),
        Err(LogError::BadSignature(2))
    );

    let mut time = entries[1].clone();
    time.at_tai_ns += 1;
    assert_eq!(
        verify_next(&head1, &time, &trust),
        Err(LogError::BadSignature(2))
    );

    // Re-signed by a node that is not a cluster member.
    let rogue = signer("rogue", 9);
    let forged = LogEntry::create(&head1, Fence::new(1, node("rogue")), body(1), &rogue).unwrap();
    assert!(matches!(
        verify_next(&head1, &forged, &trust),
        Err(LogError::UnknownSigner { .. })
    ));
    // Claiming a trusted node's fence with a foreign key.
    let impostor = LogEntry::create(&head1, Fence::new(1, node("b")), body(1), &rogue).unwrap();
    assert_eq!(
        verify_next(&head1, &impostor, &trust),
        Err(LogError::BadSignature(2))
    );

    // Gaps, repeats and broken links.
    assert!(matches!(
        verify_next(&head1, &entries[2], &trust),
        Err(LogError::BadSequence {
            expected: 2,
            got: 3
        })
    ));
    assert!(matches!(
        verify_next(&head1, &entries[0], &trust),
        Err(LogError::BadSequence { .. })
    ));
    let other = chain(2);
    let mut relinked = other[1].clone();
    relinked.prev_hash = [7; 32];
    assert_eq!(
        verify_next(&head1, &relinked, &trust),
        Err(LogError::BrokenChain(2))
    );

    // A fence going backwards along the chain.
    let b = signer("b", 2);
    let newer = LogEntry::create(&head1, Fence::new(5, node("b")), body(1), &b).unwrap();
    let head2 = ChainHead::of(&newer);
    let s = signer("a", 1);
    let older = LogEntry::create(&head2, Fence::new(4, node("a")), body(2), &s).unwrap();
    assert_eq!(
        verify_next(&head2, &older, &trust),
        Err(LogError::FenceRegression(3))
    );

    // A follower refuses an entry claiming a newer fence than its leader.
    let mut log = MemLog::default();
    let mut follow = FollowerSession::new(Fence::new(1, node("a")), ChainHead::genesis());
    follow.on_resume(&mut log, 0).unwrap();
    let ahead =
        LogEntry::create(&ChainHead::genesis(), Fence::new(5, node("b")), body(0), &b).unwrap();
    assert!(matches!(
        follow.accept(&mut log, &trust, &ahead),
        Err(ReplicationError::Log(LogError::FenceAhead(1)))
    ));
    assert_eq!(log.len(), 0);
}

#[test]
fn f_sys_04_tampering_anywhere_in_a_stored_log_is_detected() {
    let entries = chain(10);
    let trust = trust_ab();
    assert!(verify_chain(&ChainHead::genesis(), &entries, &trust).is_ok());

    let mut changed = entries.clone();
    changed[4].payload = b"edited".to_vec();
    assert!(verify_chain(&ChainHead::genesis(), &changed, &trust).is_err());

    let mut removed = entries.clone();
    removed.remove(6);
    assert!(verify_chain(&ChainHead::genesis(), &removed, &trust).is_err());

    let mut reordered = entries.clone();
    reordered.swap(2, 3);
    assert!(verify_chain(&ChainHead::genesis(), &reordered, &trust).is_err());

    let mut untrusted = TrustedSigners::new();
    untrusted
        .insert(node("a"), &key(2).verifying_key().to_bytes())
        .unwrap();
    assert!(verify_chain(&ChainHead::genesis(), &entries, &untrusted).is_err());
}

#[test]
fn f_sys_04_wire_messages_round_trip_and_reject_garbage() {
    let entries = chain(2);
    let message = PeerMessage::Entries {
        entries: entries.iter().map(WireEntry::from).collect(),
    };
    let text = message.encode().unwrap();
    assert_eq!(PeerMessage::decode(&text).unwrap(), message);
    assert!(PeerMessage::decode("{\"type\":\"nonsense\"}").is_err());
    let mut wire = WireEntry::from(&entries[0]);
    wire.signature = "zz".into();
    assert!(LogEntry::try_from(&wire).is_err());
    assert!(PeerMessage::decode(
        "{\"type\":\"hello\",\"fence\":{\"epoch\":1,\"node\":\"bad node\"},\"head_seq\":0}"
    )
    .is_err());
}
