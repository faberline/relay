// SPEC-MANAGED: tech-design/logic/adopt-raft-host-relaystatemachine-auto-mode-ha-drop-hand-rolled.md#unit-test
// HANDWRITE-BEGIN gap="missing-generator:unit-test:8ae02b04" tracker="pending-tracker" reason="Restart-recovery tests over RelayRaft (#544): a single-node group restarted from its data dir rejoins with applied state intact and accepts new proposes (no double-apply); and the resurrection case — acked work already trimmed by delete-on-ack is NOT re-appended by cold replay thanks to the fsynced applied-index floor."
//! Restart recovery over the raft-runtime stack (#544): the fsynced
//! applied-index marker is relay's honest floor. The engine is delete-on-ack
//! with a bounded dedupe window, so cold-replaying the resident committed raft
//! log without a floor would resurrect acked (already-trimmed) work; with it,
//! a restarted node rejoins exactly where it left off.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use raft_runtime::Membership;
use relay::{PubCommand, Relay, RelayCoreConfig, RelayRaft};

fn sole_voter() -> Membership {
    Membership {
        voters: vec![0],
        learners: vec![],
    }
}

fn cmd(id: &str) -> PubCommand {
    PubCommand {
        subject: "s".to_string(),
        message_id: id.to_string(),
        payload: serde_json::json!({ "m": id }),
        headers: Default::default(),
        priority: relay::DEFAULT_PRIORITY,
        not_before: None,
        appended_at: Utc::now(),
    }
}

fn disk_engine(dir: &Path, segment_bytes: u64) -> Arc<Relay> {
    Arc::new(Relay::new(RelayCoreConfig {
        data_dir: dir.join("relay").to_str().unwrap().to_string(),
        segment_bytes,
        ..RelayCoreConfig::default()
    }))
}

fn spawn(engine: Arc<Relay>, dir: &Path) -> RelayRaft {
    RelayRaft::spawn(
        engine,
        &dir.join("raft"),
        0,
        sole_voter(),
        HashMap::new(),
        RelayRaft::host_config(1024),
    )
    .unwrap()
}

/// AC3 (idempotency + floor): a restarted sole-voter node recovers its engine
/// from disk and its applied floor from the marker, performs NO double-apply
/// of the resident committed log, and resumes proposing at the next index.
#[tokio::test]
async fn restart_rejoins_with_applied_state_intact() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = disk_engine(dir.path(), 134_217_728);
        let raft = spawn(engine.clone(), dir.path());
        for i in 0..3 {
            let (_, out) = raft.publish(&cmd(&format!("m{i}"))).await.unwrap();
            assert!(!out.expect("outcome").deduped);
        }
        assert_eq!(raft.applied_index(), 3);
        assert_eq!(engine.log_len("s").unwrap(), 3);
    } // drop = restart (host tasks abort; engine files stay)

    let engine = disk_engine(dir.path(), 134_217_728);
    let raft = spawn(engine.clone(), dir.path());
    // The marker survived: the floor is back before any replay runs.
    assert_eq!(raft.applied_index(), 3, "applied floor recovered");
    assert_eq!(
        engine.log_len("s").unwrap(),
        3,
        "no double-apply on restart"
    );

    // The group resumes: the next publish lands at raft index 4 / engine seq 3.
    let (idx, out) = raft.publish(&cmd("m99")).await.unwrap();
    assert_eq!(idx, 4);
    let out = out.expect("outcome");
    assert!(!out.deduped);
    assert_eq!(out.seq, 3);
    assert_eq!(engine.log_len("s").unwrap(), 4);
}

/// AC3 (the resurrection case — the marker is load-bearing): with tiny
/// segments, acking everything trims the acked entries from disk AND from the
/// recovered dedupe window. The committed raft log still holds those
/// publishes; only the persisted applied floor stops cold replay from
/// re-appending (resurrecting) them after a restart.
#[tokio::test]
async fn acked_work_is_not_resurrected_by_cold_replay() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = disk_engine(dir.path(), 1); // 1-byte segments: one entry each
        let raft = spawn(engine.clone(), dir.path());
        for i in 0..5 {
            raft.publish(&cmd(&format!("m{i}"))).await.unwrap();
        }
        // Drain the queue locally (leases are node-local): ack all 5, which
        // persists the watermark and drops the fully-acked segments.
        let now = Utc::now();
        while let Some(l) = engine.lease("s", "w", now).unwrap() {
            assert!(engine.ack("s", &l.lease_id, Some(l.epoch)).unwrap());
        }
        assert_eq!(engine.log_len("s").unwrap(), 5);
    } // drop = restart

    let engine = disk_engine(dir.path(), 1);
    let raft = spawn(engine.clone(), dir.path());
    assert_eq!(raft.applied_index(), 5, "applied floor recovered");

    // A new-term publish forces the recovered backlog to (re-)commit; the
    // floor makes replay skip entries 1..=5. Without it, m0..m3 (trimmed from
    // disk and dedupe) would re-append here and log_len would exceed 6.
    let (idx, out) = raft.publish(&cmd("m100")).await.unwrap();
    assert_eq!(idx, 6);
    assert!(!out.expect("outcome").deduped);
    assert_eq!(
        engine.log_len("s").unwrap(),
        6,
        "acked work must not be resurrected by cold replay"
    );

    // Only the new entry is leasable — the acked five stay acked.
    let now = Utc::now();
    let lease = engine.lease("s", "w", now).unwrap().expect("new entry");
    assert_eq!(lease.seq, 5);
    assert!(engine.lease("s", "w2", now).unwrap().is_none());
}

/// AC1 (multi-voter hard-state restart): prove that voters restarted from their
/// own data dir keep committed state without double-apply and rejoin the group,
/// keeping at most one leader per term and losing no committed publish across
/// a restart of the leader and of a follower.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restarted_voters_rejoin_with_hard_state_intact() {
    let _guard = crate::cluster::CLUSTER_TEST_LOCK.lock().await;
    let mut c = crate::cluster::Cluster::prepare(3).await;
    c.start_all(1024);

    // Step 1: elect a leader, publish 3 messages through it, wait until every
    // node's applied_index() is 3.
    let leader = c.wait_leader().await;
    for i in 0..3 {
        let msg = format!("m{i}");
        let (_, out) = c
            .raft(leader)
            .publish(&crate::cluster::cmd(&msg))
            .await
            .unwrap();
        assert!(
            !out.expect("step 1: outcome").deduped,
            "step 1: node {leader} publish {msg} not deduped"
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let all_applied = c.live().all(|(_, n)| n.raft.applied_index() == 3);
        if all_applied {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "step 1: node 0..3 did not reach applied_index 3 within deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    for (id, n) in c.live() {
        assert_eq!(
            n.raft.applied_index(),
            3,
            "step 1: node {id} applied_index is 3"
        );
    }

    // Step 2, follower restart: kill one follower and start_node it again from
    // the same dir. Within a bounded deadline its applied_index() is back to 3,
    // its engine log_len("s") is 3 (no double-apply), and a leader is still
    // known on all three nodes.
    let follower = c.live().map(|(i, _)| i).find(|i| *i != leader).unwrap();
    c.kill(follower).await;
    c.start_node(follower, 1024);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let follower_applied = c.raft(follower).applied_index() == 3;
        let follower_log = c.raft(follower).relay().log_len("s").unwrap() == 3;
        let mut all_know_leader = true;
        for (_, n) in c.live() {
            if n.raft.leader().await.is_none() {
                all_know_leader = false;
                break;
            }
        }
        if follower_applied && follower_log && all_know_leader {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "step 2: node {follower} restart did not converge within deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        c.raft(follower).applied_index(),
        3,
        "step 2: node {follower} applied_index is 3"
    );
    assert_eq!(
        c.raft(follower).relay().log_len("s").unwrap(),
        3,
        "step 2: node {follower} engine log_len is 3 (no double-apply)"
    );
    for (id, n) in c.live() {
        assert!(
            n.raft.leader().await.is_some(),
            "step 2: node {id} knows a leader"
        );
    }

    // Step 3, leader restart: kill the leader and start_node it again from the
    // same dir (do not wait for another leader first). Within a bounded deadline
    // the group has exactly one node for which is_leader() is true. Publish 1
    // more message through a live node: it commits, and every node converges to
    // applied_index() == 4 and log_len("s") == 4, with the same message ids on
    // every node and no duplicate.
    c.kill(leader).await;
    c.start_node(leader, 1024);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let step3_leader = loop {
        let mut leaders = Vec::new();
        for (id, n) in c.live() {
            if n.raft.is_leader().await {
                leaders.push(id);
            }
        }
        if leaders.len() == 1 {
            break leaders[0];
        }
        assert!(
            std::time::Instant::now() < deadline,
            "step 3: node {leader} restart did not reach exactly one leader within deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(
        c.raft(step3_leader).is_leader().await,
        "step 3: node {step3_leader} is leader"
    );

    let (_, out) = c
        .raft(step3_leader)
        .publish(&crate::cluster::cmd("m3"))
        .await
        .unwrap();
    assert!(
        !out.expect("step 3: outcome").deduped,
        "step 3: node {step3_leader} publish m3 not deduped"
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let converged = c
            .live()
            .all(|(_, n)| n.raft.applied_index() == 4 && n.raft.relay().log_len("s").unwrap() == 4);
        if converged {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "step 3: node 0..3 did not converge to applied_index 4 and log_len 4 within deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let want_ids = vec![
        "m0".to_string(),
        "m1".to_string(),
        "m2".to_string(),
        "m3".to_string(),
    ];
    for (id, n) in c.live() {
        assert_eq!(
            n.raft.applied_index(),
            4,
            "step 3: node {id} applied_index is 4"
        );
        assert_eq!(
            n.raft.relay().log_len("s").unwrap(),
            4,
            "step 3: node {id} log_len is 4"
        );
        assert_eq!(
            c.message_ids(id),
            want_ids,
            "step 3: node {id} message ids match without duplicate"
        );
    }

    // Step 4: restart all three nodes one after another, then wait_leader, and
    // publish 1 more message: every node reaches applied_index() == 5 and
    // log_len("s") == 5. An already published message_id published again is
    // reported deduped or does not grow log_len.
    for id in 0..3 {
        c.kill(id).await;
        c.start_node(id, 1024);
    }
    let step4_leader = c.wait_leader().await;

    let (_, out) = c
        .raft(step4_leader)
        .publish(&crate::cluster::cmd("m4"))
        .await
        .unwrap();
    assert!(
        !out.expect("step 4: outcome").deduped,
        "step 4: node {step4_leader} publish m4 not deduped"
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let converged = c
            .live()
            .all(|(_, n)| n.raft.applied_index() == 5 && n.raft.relay().log_len("s").unwrap() == 5);
        if converged {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "step 4: node 0..3 did not converge to applied_index 5 and log_len 5 within deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    for (id, n) in c.live() {
        assert_eq!(
            n.raft.applied_index(),
            5,
            "step 4: node {id} applied_index is 5"
        );
        assert_eq!(
            n.raft.relay().log_len("s").unwrap(),
            5,
            "step 4: node {id} log_len is 5"
        );
    }

    let (_, dup_out) = c
        .raft(step4_leader)
        .publish(&crate::cluster::cmd("m0"))
        .await
        .unwrap();
    let is_deduped = dup_out.as_ref().is_some_and(|o| o.deduped);
    // Bounded check: log_len does not grow beyond 5 on any node
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    for (id, n) in c.live() {
        assert!(
            is_deduped || n.raft.relay().log_len("s").unwrap() == 5,
            "step 4: node {id} duplicate publish reported deduped or log_len stayed 5"
        );
        assert_eq!(
            n.raft.relay().log_len("s").unwrap(),
            5,
            "step 4: node {id} log_len did not grow on duplicate"
        );
    }

    c.shutdown().await;
}
// HANDWRITE-END
