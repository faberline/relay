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

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::cluster::{cmd, Cluster, CLUSTER_TEST_LOCK};

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

/// Test 1: Reclaim, reassignment, and stale owner fencing.
///
/// Publish one message, lease it on replica A at time t.
/// RelayRaft::reconcile(t + 1s) reclaims 0 and a lease on replica B at t + 1s returns None.
/// reconcile(t + 31s) (past the 30s default lease_ttl_ms) reclaims 1.
/// A lease on replica B at a time past the reclaim and the 1s default redeliver_backoff_ms
/// returns the same seq with epoch strictly greater than the first lease and executor_node == B.
/// After the reassignment, replica A's ack, heartbeat, and release with the old lease_id and epoch
/// return not accepted (false / None / false). Replica B's ack with the new lease is accepted,
/// and every live node's committed_offset("s") converges to committed_seq == 0 within a bounded deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replicated_lease_reclaim_reassignment_and_stale_owner_fencing() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    c.start_all(1024);
    let leader = c.wait_leader().await;
    let replica_a = 0;
    let replica_b = 1;

    c.raft(leader).publish(&cmd("m0")).await.unwrap();
    c.wait_converged(1).await;

    let t = chrono::Utc::now();
    let lease_a = c
        .raft(replica_a)
        .lease("s".into(), "worker-a".into(), t)
        .await
        .unwrap()
        .expect("replica A grants initial lease");
    assert_eq!(
        lease_a.executor_node, replica_a as u64,
        "replica {replica_a} lease {lease_a:?} executor_node matches replica A"
    );

    let t_plus_1s = t + chrono::Duration::seconds(1);
    let reclaimed_early = c.raft(replica_a).reconcile(t_plus_1s).await.unwrap();
    assert_eq!(
        reclaimed_early, 0,
        "replica {replica_a} lease {lease_a:?} reconcile(t + 1s) reclaims 0"
    );

    let lease_b_early = c
        .raft(replica_b)
        .lease("s".into(), "worker-b".into(), t_plus_1s)
        .await
        .unwrap();
    assert!(
        lease_b_early.is_none(),
        "replica {replica_b} lease on replica B at t + 1s returns None while replica {replica_a} holds lease {lease_a:?}"
    );

    let t_plus_31s = t + chrono::Duration::seconds(31);
    let reclaimed_expired = c.raft(replica_a).reconcile(t_plus_31s).await.unwrap();
    assert_eq!(
        reclaimed_expired, 1,
        "replica {replica_a} lease {lease_a:?} reconcile(t + 31s) reclaims 1"
    );

    let t_plus_33s = t + chrono::Duration::seconds(33);
    let lease_b = c
        .raft(replica_b)
        .lease("s".into(), "worker-b".into(), t_plus_33s)
        .await
        .unwrap()
        .expect("replica B grants reassigned lease after reclaim and backoff");
    assert_eq!(
        lease_b.seq,
        lease_a.seq,
        "replica {replica_b} lease {lease_b:?} returns same seq as replica {replica_a} lease {lease_a:?}"
    );
    assert!(
        lease_b.epoch > lease_a.epoch,
        "replica {replica_b} lease {lease_b:?} epoch ({}) strictly greater than replica {replica_a} lease {lease_a:?} epoch ({})",
        lease_b.epoch,
        lease_a.epoch
    );
    assert_eq!(
        lease_b.executor_node, replica_b as u64,
        "replica {replica_b} lease {lease_b:?} executor_node matches replica B"
    );

    let (ack_a_accepted, _) = c
        .raft(replica_a)
        .ack(
            "s".into(),
            lease_a.lease_id.clone(),
            lease_a.epoch,
            t_plus_33s,
        )
        .await
        .unwrap();
    assert!(
        !ack_a_accepted,
        "replica {replica_a} old lease {lease_a:?} ack returns false after reassignment to replica {replica_b} lease {lease_b:?}"
    );

    let hb_a = c
        .raft(replica_a)
        .heartbeat(
            "s".into(),
            lease_a.lease_id.clone(),
            lease_a.epoch,
            t_plus_33s,
        )
        .await
        .unwrap();
    assert!(
        hb_a.is_none(),
        "replica {replica_a} old lease {lease_a:?} heartbeat returns None after reassignment to replica {replica_b} lease {lease_b:?}"
    );

    let rel_a = c
        .raft(replica_a)
        .release(
            "s".into(),
            lease_a.lease_id.clone(),
            lease_a.epoch,
            t_plus_33s,
        )
        .await
        .unwrap();
    assert!(
        !rel_a,
        "replica {replica_a} old lease {lease_a:?} release returns false after reassignment to replica {replica_b} lease {lease_b:?}"
    );

    let (ack_b_accepted, committed) = c
        .raft(replica_b)
        .ack(
            "s".into(),
            lease_b.lease_id.clone(),
            lease_b.epoch,
            t_plus_33s,
        )
        .await
        .unwrap();
    assert!(
        ack_b_accepted,
        "replica {replica_b} new lease {lease_b:?} ack is accepted"
    );
    assert_eq!(
        committed
            .expect("committed offset for replica B lease")
            .committed_seq,
        0,
        "replica {replica_b} new lease {lease_b:?} committed_seq converges to 0"
    );

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
        assert!(
            Instant::now() < deadline,
            "replica {replica_b} new lease {lease_b:?} committed ack did not converge across live nodes"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    c.shutdown().await;
}

/// Test 2: Across a leader change.
///
/// Lease on a follower, kill the leader, wait_leader for a new one,
/// then a committed reconcile past expiry through a live node and a re-lease on
/// another live node give a strictly higher epoch. The old lease's ack from its original
/// executor is rejected, and the new owner's ack is accepted.
/// At-least-once: publish a second message before the kill; after the failover both
/// messages are leased and acked by some live replica, and no acked message is leased again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replicated_lease_reclaim_across_leader_failover_preserves_fencing() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    c.start_all(1024);
    let initial_leader = c.wait_leader().await;

    let followers: Vec<usize> = c
        .live()
        .map(|(i, _)| i)
        .filter(|i| *i != initial_leader)
        .collect();
    let follower_a = followers[0];
    let follower_b = followers[1];

    c.raft(initial_leader)
        .publish(&cmd("m-failover-1"))
        .await
        .unwrap();
    c.raft(initial_leader)
        .publish(&cmd("m-failover-2"))
        .await
        .unwrap();
    c.wait_converged(2).await;

    let t0 = chrono::Utc::now();
    let lease_1 = c
        .raft(follower_a)
        .lease("s".into(), "worker-fa".into(), t0)
        .await
        .unwrap()
        .expect("follower A grants lease 1");
    assert_eq!(
        lease_1.executor_node, follower_a as u64,
        "replica {follower_a} lease_1 {lease_1:?} executor_node matches follower A"
    );

    c.kill(initial_leader).await;
    let new_leader = c.wait_leader().await;
    assert_ne!(
        new_leader,
        initial_leader,
        "replica {new_leader} is a new leader distinct from killed leader {initial_leader} for lease_1 {lease_1:?}"
    );

    let t_past_expiry = t0 + chrono::Duration::seconds(31);
    let reclaimed = c.raft(follower_a).reconcile(t_past_expiry).await.unwrap();
    assert!(
        reclaimed >= 1,
        "replica {follower_a} reconcile past expiry for lease_1 {lease_1:?} reclaimed {reclaimed}"
    );

    let t_re_lease = t0 + chrono::Duration::seconds(33);
    let lease_1_new = c
        .raft(follower_b)
        .lease("s".into(), "worker-fb".into(), t_re_lease)
        .await
        .unwrap()
        .expect("follower B grants re-lease");
    assert_eq!(
        lease_1_new.seq,
        lease_1.seq,
        "replica {follower_b} lease_1_new {lease_1_new:?} seq matches replica {follower_a} lease_1 {lease_1:?}"
    );
    assert!(
        lease_1_new.epoch > lease_1.epoch,
        "replica {follower_b} lease_1_new {lease_1_new:?} epoch ({}) strictly higher than replica {follower_a} lease_1 {lease_1:?} epoch ({})",
        lease_1_new.epoch,
        lease_1.epoch
    );
    assert_eq!(
        lease_1_new.executor_node, follower_b as u64,
        "replica {follower_b} lease_1_new {lease_1_new:?} executor_node matches follower B"
    );

    let (old_ack_ok, _) = c
        .raft(follower_a)
        .ack(
            "s".into(),
            lease_1.lease_id.clone(),
            lease_1.epoch,
            t_re_lease,
        )
        .await
        .unwrap();
    assert!(
        !old_ack_ok,
        "replica {follower_a} old lease_1 {lease_1:?} ack rejected after reassignment to replica {follower_b} lease_1_new {lease_1_new:?}"
    );

    let (new_ack_ok, _) = c
        .raft(follower_b)
        .ack(
            "s".into(),
            lease_1_new.lease_id.clone(),
            lease_1_new.epoch,
            t_re_lease,
        )
        .await
        .unwrap();
    assert!(
        new_ack_ok,
        "replica {follower_b} new lease_1_new {lease_1_new:?} ack accepted"
    );

    let lease_2 = c
        .raft(follower_a)
        .lease("s".into(), "worker-fa-2".into(), t_re_lease)
        .await
        .unwrap()
        .expect("live replica follower A leases second message");
    assert_ne!(
        lease_2.seq, lease_1.seq,
        "replica {follower_a} lease_2 {lease_2:?} seq is different from lease_1 {lease_1:?}"
    );

    let (ack_2_ok, _) = c
        .raft(follower_a)
        .ack(
            "s".into(),
            lease_2.lease_id.clone(),
            lease_2.epoch,
            t_re_lease,
        )
        .await
        .unwrap();
    assert!(
        ack_2_ok,
        "replica {follower_a} lease_2 {lease_2:?} ack accepted"
    );

    let empty_a = c
        .raft(follower_a)
        .lease("s".into(), "worker-fa".into(), t_re_lease)
        .await
        .unwrap();
    assert!(
        empty_a.is_none(),
        "replica {follower_a} lease returns None; no acked message leased again (lease_1: {lease_1:?}, lease_2: {lease_2:?})"
    );

    let empty_b = c
        .raft(follower_b)
        .lease("s".into(), "worker-fb".into(), t_re_lease)
        .await
        .unwrap();
    assert!(
        empty_b.is_none(),
        "replica {follower_b} lease returns None; no acked message leased again (lease_1: {lease_1:?}, lease_2: {lease_2:?})"
    );

    c.shutdown().await;
}

/// Test 3: Concurrent grants.
///
/// Publish N >= 6 messages, then lease from all three replicas at the same time
/// (tokio::join! or spawned tasks, N lease calls in total).
/// No (seq, epoch) pair is granted twice, every granted lease's executor_node is the
/// replica that asked, and the number of grants is at most N.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_lease_grants_across_all_replicas_are_partitioned_and_bounded() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    c.start_all(1024);
    let leader = c.wait_leader().await;

    let n = 6;
    for i in 0..n {
        let (_, out) = c
            .raft(leader)
            .publish(&cmd(&format!("m-conc-{i}")))
            .await
            .unwrap();
        assert!(
            !out.expect("publish outcome").deduped,
            "replica leader {leader} publish m-conc-{i} lease preparation not deduped"
        );
    }
    c.wait_converged(n as u64).await;

    let now = chrono::Utc::now();
    let mut handles = Vec::new();
    for i in 0..n {
        let replica = i % 3;
        let raft = Arc::clone(c.raft(replica));
        let consumer_id = format!("worker-{replica}-{i}");
        handles.push(tokio::spawn(async move {
            let lease_opt = raft.lease("s".into(), consumer_id, now).await.unwrap();
            (replica, lease_opt)
        }));
    }

    let mut grants = Vec::new();
    for h in handles {
        let (replica, lease_opt) = h.await.unwrap();
        if let Some(lease) = lease_opt {
            grants.push((replica, lease));
        }
    }

    assert!(
        grants.len() <= n,
        "replica group all replicas leases {grants:?} count ({}) at most N={n}",
        grants.len()
    );
    assert_eq!(
        grants.len(),
        n,
        "replica group all replicas leases {grants:?} count matches N={n}"
    );

    let mut seen = std::collections::HashSet::new();
    for (replica, lease) in &grants {
        let pair = (lease.seq, lease.epoch);
        assert!(
            seen.insert(pair),
            "replica {replica} lease {lease:?} duplicate (seq, epoch) pair {pair:?} granted"
        );
    }

    for (replica, lease) in &grants {
        assert_eq!(
            lease.executor_node, *replica as u64,
            "replica {replica} lease {lease:?} executor_node matches requesting replica"
        );
    }

    c.shutdown().await;
}

/// Observable oracle for capability claim `real-h2c-raft-cluster-smoke`:
/// a three-node `RelayRaft` group on real h2c listeners elects one leader,
/// commits a publish that every node's engine applies, rejects direct follower
/// `/raft/publish` with 421 not-leader, and survives one leader kill with
/// both committed messages intact on both survivors.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_h2c_raft_cluster_smoke() {
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
    assert_eq!(
        leaders, 1,
        "node {leader}: exactly one leader elected across group"
    );

    // Leader publish: outcome not deduped, converged to 1, present on all three nodes.
    let (_, outcome) = c.raft(leader).publish(&cmd("m1")).await.unwrap();
    let outcome = outcome.unwrap_or_else(|| panic!("node {leader}: leader claims apply outcome"));
    assert!(
        !outcome.deduped,
        "node {leader}: publish outcome not deduped"
    );
    c.wait_converged(1).await;
    for (i, _) in c.live() {
        assert!(
            c.message_ids(i).contains(&"m1".to_string()),
            "node {i}: message_ids contains 'm1'"
        );
    }

    // Direct POST to a follower's /raft/publish over h2c asserts 421 with error == "not-leader".
    let follower = c.live().map(|(i, _)| i).find(|i| *i != leader).unwrap();
    let resp = c
        .client
        .post(format!("{}/raft/publish", c.urls[&(follower as u64)]))
        .body(serde_json::to_vec(&cmd("m-direct")).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::MISDIRECTED_REQUEST,
        "node {follower}: direct /raft/publish returns 421"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"], "not-leader",
        "node {follower}: direct /raft/publish error is not-leader"
    );

    // Kill the leader, wait_leader returns a different node, publish commits,
    // and both messages are in message_ids on both survivors.
    c.kill(leader).await;
    let new_leader = c.wait_leader().await;
    assert_ne!(
        new_leader, leader,
        "node {new_leader}: new leader differs from killed leader {leader}"
    );
    let (_, out) = c.raft(new_leader).publish(&cmd("m2")).await.unwrap();
    let out = out.unwrap_or_else(|| panic!("node {new_leader}: second publish outcome claimed"));
    assert!(
        !out.deduped,
        "node {new_leader}: second publish outcome not deduped"
    );
    c.wait_converged(2).await;

    for (i, _) in c.live() {
        let ids = c.message_ids(i);
        assert!(
            ids.contains(&"m1".to_string()),
            "survivor node {i}: message_ids contains 'm1'"
        );
        assert!(
            ids.contains(&"m2".to_string()),
            "survivor node {i}: message_ids contains 'm2'"
        );
    }
    println!("real h2c raft cluster smoke");
    c.shutdown().await;
}
// HANDWRITE-END
