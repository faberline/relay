// SPEC-MANAGED: tech-design/logic/adopt-raft-host-relaystatemachine-auto-mode-ha-drop-hand-rolled.md#unit-test
// HANDWRITE-BEGIN gap="missing-generator:unit-test:e331737f" tracker="pending-tracker" reason="raft-runtime adoption cluster tests (#544): an in-process 3-node RelayRaft group over real h2c listeners — election, leader publish applied on every engine, follower publish forwarded by the host, direct follower /raft/publish answers 421, leader kill re-elects with no committed loss; and the snapshot path — a small SnapshotPolicy threshold compacts the leader log so a late-started fresh node catches up via InstallSnapshot."
//! raft-runtime cluster integration (#544): 3 `RelayRaft` nodes over real h2c.
//!
//! Replaces the old hand-rolled-driver test: the group now runs the shared
//! [`raft_runtime::RaftHost`] (relay supplies only [`relay::RelayStateMachine`]),
//! publishes are multi-subject [`PubCommand`]s, a follower publish is
//! *forwarded* to the leader by the host (the old driver only redirected), and
//! a recovered node catches up before a second leader loss, and the
//! snapshot/compaction path is real — a fresh node catches up via
//! InstallSnapshot instead of full log replay.

use std::time::{Duration, Instant};

use crate::cluster::{cmd, Cluster, CLUSTER_TEST_LOCK};

/// AC2 + failover: exactly one leader; a leader publish applies on every
/// node's engine; a follower publish is forwarded by the host; a direct
/// follower `/raft/publish` answers 421 not-leader; the raft path stays
/// idempotent per message_id; killing the leader re-elects with no committed
/// loss and the group keeps accepting publishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_node_group_elects_replicates_forwards_and_fails_over() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    c.start_all(1024);

    // Exactly one leader.
    let leader = c.wait_leader().await;
    let mut leaders = 0;
    for (_, n) in c.live() {
        if n.raft.is_leader().await {
            leaders += 1;
        }
    }
    assert_eq!(leaders, 1, "exactly one leader");

    // Leader publish: applied on ALL engines, outcome claimed (read-your-write).
    let (_, outcome) = c.raft(leader).publish(&cmd("a")).await.unwrap();
    let outcome = outcome.expect("leader claims its apply outcome");
    assert!(!outcome.deduped);
    c.wait_converged(1).await;

    // The raft path is idempotent per message_id: re-publishing "a" dedupes.
    let (_, dup) = c.raft(leader).publish(&cmd("a")).await.unwrap();
    assert!(dup.expect("outcome").deduped, "duplicate publish dedupes");

    // Follower publish: the HOST forwards it to the leader and resolves once
    // the follower's own engine applied it.
    let follower = c.live().map(|(i, _)| i).find(|i| *i != leader).unwrap();
    let (_, fw) = c.raft(follower).publish(&cmd("b")).await.unwrap();
    assert!(!fw.expect("follower claims its apply outcome").deduped);
    c.wait_converged(2).await;

    // A direct POST to a follower's /raft/publish answers 421 + leader hint.
    let resp = c
        .client
        .post(format!("{}/raft/publish", c.urls[&(follower as u64)]))
        .body(serde_json::to_vec(&cmd("x")).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::MISDIRECTED_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "not-leader");

    // Kill the leader -> survivors re-elect and keep the committed entries.
    c.kill(leader).await;
    let new_leader = c.wait_leader().await;
    assert_ne!(new_leader, leader);
    let (_, out) = c.raft(new_leader).publish(&cmd("c")).await.unwrap();
    assert!(!out.expect("outcome").deduped);
    c.wait_converged(3).await;

    // Recover the first leader from its durable raft directory, prove that it
    // catches up, then remove the current leader and commit through the next
    // elected primary.
    c.start_node(leader, 1024);
    c.wait_converged(3).await;
    for want in ["a", "b", "c"] {
        assert!(
            c.message_ids(leader).contains(&want.to_string()),
            "recovered node holds '{want}'"
        );
    }
    let second_leader = c.wait_leader().await;
    c.kill(second_leader).await;
    let third_leader = c.wait_leader().await;
    assert_ne!(third_leader, second_leader);
    let (_, out) = c.raft(third_leader).publish(&cmd("d")).await.unwrap();
    assert!(!out.expect("outcome").deduped);
    c.wait_converged(4).await;

    for (i, _) in c.live() {
        let got = c.message_ids(i);
        for want in ["a", "b", "c", "d"] {
            assert!(got.contains(&want.to_string()), "node {i} holds '{want}'");
        }
    }
    c.shutdown().await;
}

/// Relay's effectful delivery lifecycle is authoritative Raft state: a lease
/// granted through one follower is visible everywhere, cannot be leased again,
/// and can only be completed by the executor replica holding its fencing token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lease_and_ack_are_committed_and_executor_fenced() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    c.start_all(1024);
    let leader = c.wait_leader().await;
    let executor = c.live().map(|(i, _)| i).find(|i| *i != leader).unwrap();
    let other = c.live().map(|(i, _)| i).find(|i| *i != executor).unwrap();

    c.raft(leader).publish(&cmd("owned")).await.unwrap();
    c.wait_converged(1).await;
    let lease = c
        .raft(executor)
        .lease("s".into(), "worker-a".into(), chrono::Utc::now())
        .await
        .unwrap()
        .expect("committed lease");
    assert_eq!(lease.executor_node, executor as u64);

    // The next committed lease command observes the first one on every state
    // machine, so no second replica can assign the same work.
    let conflicting = c
        .raft(other)
        .lease("s".into(), "worker-b".into(), chrono::Utc::now())
        .await
        .unwrap();
    assert!(conflicting.is_none(), "no cross-replica dual lease");

    // Correct epoch but wrong executor is fenced by committed owner identity.
    let (accepted, _) = c
        .raft(other)
        .ack(
            "s".into(),
            lease.lease_id.clone(),
            lease.epoch,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert!(!accepted, "non-owner executor cannot acknowledge");

    let (accepted, committed) = c
        .raft(executor)
        .ack("s".into(), lease.lease_id, lease.epoch, chrono::Utc::now())
        .await
        .unwrap();
    assert!(accepted);
    assert_eq!(committed.expect("watermark").committed_seq, 0);

    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if c.live().all(|(_, n)| {
            n.raft
                .relay()
                .committed_offset("s")
                .unwrap()
                .is_some_and(|offset| offset.committed_seq == 0)
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "committed ack did not converge");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    c.shutdown().await;
}

/// AC4: with a small SnapshotPolicy threshold the leader compacts its raft
/// log, so a node that starts late (empty engine, empty raft state) cannot be
/// caught up by AppendEntries alone — it restores via InstallSnapshot (the
/// live un-acked engine dump) and then tails the remaining entries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_node_catches_up_via_install_snapshot() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    // Node 2 stays down; 2 of 3 voters still form a quorum.
    c.leave_listener_unbound_until_start(2);
    c.start_node(0, 8);
    c.start_node(1, 8);
    let leader = c.wait_leader().await;

    for i in 0..20 {
        let (_, out) = c
            .raft(leader)
            .publish(&cmd(&format!("m{i}")))
            .await
            .unwrap();
        assert!(!out.expect("outcome").deduped);
    }
    c.wait_converged(20).await;

    // Late joiner: fresh engine + fresh raft dir. The leader's log floor is
    // above index 1 (compaction every 8 applies), so catch-up REQUIRES an
    // InstallSnapshot round before AppendEntries can tail the rest.
    c.start_node(2, 8);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if c.raft(2).relay().log_len("s").unwrap() >= 20 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fresh node did not catch up via InstallSnapshot"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        c.raft(2).applied_index() >= 20,
        "snapshot restore set the applied floor"
    );
    let got = c.message_ids(2);
    for i in 0..20 {
        assert!(got.contains(&format!("m{i}")), "fresh node holds m{i}");
    }
    c.shutdown().await;
}
// HANDWRITE-END
