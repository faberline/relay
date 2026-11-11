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
use std::time::{Duration, Instant};

use chrono::Utc;
use raft_runtime::{Membership, RaftStateMachine};
use relay::{
    PubCommand, Relay, RelayCommand, RelayCoreConfig, RelayOutcome, RelayRaft, RelayStateMachine,
};

use super::raft_cluster::CLUSTER_TEST_LOCK;

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

/// AC1 & AC2: direct RelayStateMachine apply handles PubCommand variants,
/// bare legacy PubCommand JSON, RelayCommand::Publish, and advances applied_index
/// while claim_outcome returns the outcome once. Undecodable garbage bytes no-op
/// and advance the floor without stalling apply.
#[test]
fn state_machine_apply_variants_advance_and_yield_outcomes() {
    let engine = Arc::new(Relay::new(RelayCoreConfig::in_memory()));
    let sm = RelayStateMachine::new(engine.clone(), None).unwrap();
    assert_eq!(sm.applied_index(), 0);

    // 1. Bare legacy PubCommand JSON entry
    let c1 = cmd("m1");
    let c1_bytes = serde_json::to_vec(&c1).unwrap();
    sm.apply(1, &c1_bytes).unwrap();
    assert_eq!(sm.applied_index(), 1);
    assert_eq!(engine.log_len("s").unwrap(), 1);
    let outcome1 = sm.claim_outcome(1).expect("outcome for index 1");
    match outcome1 {
        RelayOutcome::Published(out) => {
            assert!(!out.deduped);
            assert_eq!(out.seq, 0);
        }
        other => panic!("expected Published outcome, got {other:?}"),
    }
    // Claiming again yields None
    assert!(sm.claim_outcome(1).is_none());

    // 2. Tagged RelayCommand::Publish entry
    let c2 = cmd("m2");
    let c2_bytes = serde_json::to_vec(&RelayCommand::Publish { command: c2 }).unwrap();
    sm.apply(2, &c2_bytes).unwrap();
    assert_eq!(sm.applied_index(), 2);
    assert_eq!(engine.log_len("s").unwrap(), 2);
    let outcome2 = sm.claim_outcome(2).expect("outcome for index 2");
    match outcome2 {
        RelayOutcome::Published(out) => {
            assert!(!out.deduped);
            assert_eq!(out.seq, 1);
        }
        other => panic!("expected Published outcome, got {other:?}"),
    }
    assert!(sm.claim_outcome(2).is_none());

    // 3. Garbage bytes entry (undecodable)
    let garbage = b"not-valid-json-command-bytes";
    sm.apply(3, garbage).unwrap();
    assert_eq!(
        engine.log_len("s").unwrap(),
        2,
        "engine unchanged after undecodable command"
    );
    assert_eq!(
        sm.applied_index(),
        3,
        "applied_index advances on no-op entry"
    );
    assert!(sm.claim_outcome(3).is_none());
}

/// AC3 & AC4: snapshot carries live queue state (unleased and leased with
/// owner + epoch) and excludes acked entries; restore into fresh engine replaces
/// state exactly and preserves the floor to the marker file.
#[test]
fn state_machine_snapshot_restore_round_trip_and_persisted_marker_floor() {
    let engine_a = Arc::new(Relay::new(RelayCoreConfig::in_memory()));
    let sm_a = RelayStateMachine::new(engine_a.clone(), None).unwrap();

    // Publish m1, m2, m3
    for (idx, id) in [(1, "m1"), (2, "m2"), (3, "m3")] {
        let bytes = serde_json::to_vec(&cmd(id)).unwrap();
        sm_a.apply(idx, &bytes).unwrap();
    }
    assert_eq!(sm_a.applied_index(), 3);
    assert_eq!(engine_a.log_len("s").unwrap(), 3);

    let now = Utc::now();
    let l1 = engine_a
        .lease_on_node("s", "worker-1", 42, now)
        .unwrap()
        .expect("lease m1");
    let l2 = engine_a
        .lease_on_node("s", "worker-1", 42, now)
        .unwrap()
        .expect("lease m2");
    let l3 = engine_a
        .lease_on_node("s", "worker-1", 42, now)
        .unwrap()
        .expect("lease m3");

    // Ack m1 so the contiguous watermark advances past seq 0 (m1 is committed and trimmed)
    assert!(engine_a
        .ack_on_node("s", &l1.lease_id, 42, l1.epoch, now)
        .unwrap());

    // Release m2 so it remains published and unleased
    assert!(engine_a
        .release_on_node("s", &l2.lease_id, 42, l2.epoch, now)
        .unwrap());

    // m3 remains leased with owner node 42 and epoch l3.epoch

    // Take snapshot of sm_a into Vec<u8>
    let mut snap_bytes = Vec::new();
    sm_a.snapshot(&mut snap_bytes).unwrap();
    assert!(!snap_bytes.is_empty());

    // Restore into fresh engine_b and sm_b created with marker path
    let dir = tempfile::tempdir().unwrap();
    let marker_path = dir.path().join("applied-floor.idx");
    let engine_b = Arc::new(Relay::new(RelayCoreConfig::in_memory()));
    let sm_b = RelayStateMachine::new(engine_b.clone(), Some(marker_path.clone())).unwrap();
    assert_eq!(sm_b.applied_index(), 0);

    sm_b.restore(&mut snap_bytes.as_slice()).unwrap();

    // B.applied_index() equals A's
    assert_eq!(sm_b.applied_index(), sm_a.applied_index());
    assert_eq!(sm_b.applied_index(), 3);

    // Live backlog has un-acked entries m2 and m3, but not acked m1
    let live = engine_b.dump_live().unwrap();
    let all_entries: Vec<_> = live.iter().flat_map(|s| &s.entries).collect();
    assert!(
        !all_entries.iter().any(|e| e.message_id == "m1"),
        "acked m1 absent from B"
    );
    assert!(
        all_entries.iter().any(|e| e.message_id == "m2"),
        "unleased m2 present in B"
    );
    assert!(
        all_entries.iter().any(|e| e.message_id == "m3"),
        "leased m3 present in B"
    );

    // m3 retains its executor owner (node 42) and epoch
    assert!(
        engine_b
            .heartbeat_on_node("s", &l3.lease_id, 42, l3.epoch, now)
            .unwrap()
            .is_some(),
        "heartbeat succeeds with restored owner node 42 and epoch"
    );
    assert!(
        engine_b
            .heartbeat_on_node("s", &l3.lease_id, 42, l3.epoch + 1, now)
            .unwrap()
            .is_none(),
        "heartbeat rejected with stale epoch"
    );
    assert!(
        engine_b
            .heartbeat_on_node("s", &l3.lease_id, 999, l3.epoch, now)
            .unwrap()
            .is_none(),
        "heartbeat rejected with wrong executor node"
    );

    // m2 is unleased and can be leased
    let fresh_l2 = engine_b
        .lease("s", "worker-2", now)
        .unwrap()
        .expect("m2 is leasable");
    assert_eq!(fresh_l2.seq, l2.seq);

    // AC4: Marker floor persistence
    // A new RelayStateMachine on the same marker reports the restored up_to as applied_index
    let engine_c = Arc::new(Relay::new(RelayCoreConfig::in_memory()));
    let sm_c = RelayStateMachine::new(engine_c, Some(marker_path)).unwrap();
    assert_eq!(
        sm_c.applied_index(),
        3,
        "new state machine recovers restored applied floor from marker"
    );
}

/// AC5: proposal retried with the same proposal id after restore is not
/// applied twice (completed_proposals cache is restored).
/// Covered directly via RelayStateMachine::apply with serialized envelope bytes
/// (matching private RelayEnvelope JSON schema), verifying completed_proposals
/// cache is restored and prevents double-application without requiring a running
/// Raft cluster/host. The replay uses a different message_id so engine dedupe
/// cannot satisfy the test on its own.
#[test]
fn state_machine_completed_proposals_restored_prevent_double_apply() {
    let engine_a = Arc::new(Relay::new(RelayCoreConfig::in_memory()));
    let sm_a = RelayStateMachine::new(engine_a.clone(), None).unwrap();

    let proposal_envelope = serde_json::json!({
        "proposal_id": {
            "node": 1,
            "session": 100,
            "sequence": 1
        },
        "command": {
            "op": "publish",
            "command": cmd("m-idempotent")
        }
    });
    let cmd_bytes = serde_json::to_vec(&proposal_envelope).unwrap();

    sm_a.apply(1, &cmd_bytes).unwrap();
    assert_eq!(sm_a.applied_index(), 1);
    assert_eq!(engine_a.log_len("s").unwrap(), 1);
    let outcome1 = sm_a.claim_outcome(1).expect("outcome on A");

    // Snapshot sm_a (carries completed_proposals)
    let mut snap_bytes = Vec::new();
    sm_a.snapshot(&mut snap_bytes).unwrap();

    // Restore into fresh engine_b
    let engine_b = Arc::new(Relay::new(RelayCoreConfig::in_memory()));
    let sm_b = RelayStateMachine::new(engine_b.clone(), None).unwrap();
    sm_b.restore(&mut snap_bytes.as_slice()).unwrap();
    assert_eq!(sm_b.applied_index(), 1);
    assert_eq!(engine_b.log_len("s").unwrap(), 1);

    // Replay the same proposal id at index 2 with a different message_id, so
    // the engine's message_id dedupe cannot mask a missing completed_proposals cache.
    let other_envelope = serde_json::json!({
        "proposal_id": {
            "node": 1,
            "session": 100,
            "sequence": 1
        },
        "command": {
            "op": "publish",
            "command": cmd("m-other")
        }
    });
    let other_bytes = serde_json::to_vec(&other_envelope).unwrap();
    sm_b.apply(2, &other_bytes).unwrap();
    // Engine must NOT have a second entry
    assert_eq!(
        engine_b.log_len("s").unwrap(),
        1,
        "proposal must not be applied twice after restore"
    );
    assert_eq!(sm_b.applied_index(), 2);

    // Cached outcome is returned via claim_outcome, not an engine dedupe
    let outcome2 = sm_b.claim_outcome(2).expect("cached outcome on B");
    match (&outcome1, &outcome2) {
        (RelayOutcome::Published(o1), RelayOutcome::Published(o2)) => {
            assert_eq!(o1.seq, o2.seq);
            assert!(
                !o2.deduped,
                "cached outcome must be A's original, not an engine dedupe"
            );
        }
        _ => panic!("outcomes mismatch: {outcome1:?} vs {outcome2:?}"),
    }
}

/// Issue 11 (durable-raft-hard-state-restore): a three-voter group restarted
/// on its own data dirs. Same shape as `Cluster` in raft_cluster.rs: one
/// pre-bound listener per node, a per-node tempdir for `relay/` and `raft/`.
const DEADLINE: Duration = Duration::from_secs(8);

struct Node {
    raft: Arc<RelayRaft>,
    serve: tokio::task::JoinHandle<()>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

struct Group {
    urls: HashMap<u64, String>,
    listeners: Vec<Option<tokio::net::TcpListener>>,
    membership: Membership,
    nodes: Vec<Option<Node>>,
    dirs: Vec<tempfile::TempDir>,
}

impl Group {
    async fn prepare(n: u64) -> Group {
        let mut listeners = Vec::new();
        let mut urls = HashMap::new();
        for id in 0..n {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            urls.insert(id, format!("http://{}", l.local_addr().unwrap()));
            listeners.push(Some(l));
        }
        Group {
            urls,
            listeners,
            membership: Membership {
                voters: (0..n).collect(),
                learners: vec![],
            },
            nodes: (0..n).map(|_| None).collect(),
            dirs: (0..n).map(|_| tempfile::tempdir().unwrap()).collect(),
        }
    }

    /// Spawn node `id` on its existing dirs and serve its peer router.
    fn start_node(&mut self, id: usize) {
        let engine = Arc::new(Relay::new(RelayCoreConfig {
            data_dir: self.dirs[id]
                .path()
                .join("relay")
                .to_string_lossy()
                .into_owned(),
            ..RelayCoreConfig::default()
        }));
        let peers: HashMap<u64, String> = self
            .urls
            .iter()
            .filter(|(k, _)| **k != id as u64)
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        let raft = Arc::new(
            RelayRaft::spawn(
                engine,
                &self.dirs[id].path().join("raft"),
                id as u64,
                self.membership.clone(),
                peers,
                RelayRaft::host_config(1024),
            )
            .unwrap(),
        );
        let app = raft.router();
        let listener = match self.listeners[id].take() {
            Some(listener) => listener,
            None => {
                let address = self.urls[&(id as u64)]
                    .strip_prefix("http://")
                    .unwrap()
                    .parse::<std::net::SocketAddr>()
                    .unwrap();
                let listener = std::net::TcpListener::bind(address).unwrap();
                listener.set_nonblocking(true).unwrap();
                tokio::net::TcpListener::from_std(listener).unwrap()
            }
        };
        let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
        let serve = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        self.nodes[id] = Some(Node {
            raft,
            serve,
            shutdown: Some(shutdown),
        });
    }

    fn start_all(&mut self) {
        for id in 0..self.nodes.len() {
            self.start_node(id);
        }
    }

    fn raft(&self, id: usize) -> &Arc<RelayRaft> {
        &self.nodes[id].as_ref().expect("node running").raft
    }

    /// Stop the node: abort its serve loop and shut its host down. Its dirs stay.
    async fn kill(&mut self, id: usize) {
        if let Some(mut n) = self.nodes[id].take() {
            n.shutdown.take();
            n.serve.abort();
            let _ = n.serve.await;
            n.raft.shutdown().await.unwrap();
        }
    }

    async fn shutdown(mut self) {
        for node in self.nodes.iter_mut() {
            if let Some(mut node) = node.take() {
                node.raft.shutdown().await.unwrap();
                if let Some(shutdown) = node.shutdown.take() {
                    let _ = shutdown.send(());
                }
                let _ = node.serve.await;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    fn live(&self) -> impl Iterator<Item = (usize, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| n.as_ref().map(|n| (i, n)))
    }

    /// Wait until exactly one live node reports `is_leader()`; return its index.
    async fn wait_single_leader(&self) -> usize {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let mut leaders = Vec::new();
            for (i, n) in self.live() {
                if n.raft.is_leader().await {
                    leaders.push(i);
                }
            }
            if leaders.len() == 1 {
                return leaders[0];
            }
            assert!(
                Instant::now() < deadline,
                "expected exactly one leader within 8 s, got {leaders:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait until every LIVE node holds at least `want` entries and all live
    /// nodes report the same applied index.
    async fn wait_converged(&self, want: u64) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let applied: Vec<u64> = self.live().map(|(_, n)| n.raft.applied_index()).collect();
            let logs_ok = self
                .live()
                .all(|(_, n)| n.raft.relay().log_len("s").unwrap() >= want);
            if logs_ok && applied.windows(2).all(|w| w[0] == w[1]) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "live nodes did not converge to {want} entries with one applied index: {applied:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn message_ids(&self, id: usize) -> Vec<String> {
        let r = self.raft(id).relay();
        let len = r.log_len("s").unwrap();
        (0..len)
            .filter_map(|seq| r.entry("s", 0, seq).unwrap().map(|e| e.message_id))
            .collect()
    }
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Test 1, follower restart: the follower comes back on its own dirs,
/// converges in order, applies nothing twice, and dedupes an old message_id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_voter_follower_restart_rejoins_without_double_apply() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut g = Group::prepare(3).await;
    g.start_all();
    let leader = g.wait_single_leader().await;
    for m in ["m0", "m1", "m2"] {
        let (_, out) = g.raft(leader).publish(&cmd(m)).await.unwrap();
        assert!(!out.expect("leader claims outcome").deduped, "publish {m}");
    }
    g.wait_converged(3).await;

    let follower = (0..3).find(|i| *i != leader).unwrap();
    g.kill(follower).await;
    let leader = g.wait_single_leader().await;
    for m in ["m3", "m4"] {
        let (_, out) = g.raft(leader).publish(&cmd(m)).await.unwrap();
        assert!(!out.expect("leader claims outcome").deduped, "publish {m}");
    }
    g.wait_converged(5).await;

    g.start_node(follower);
    g.wait_converged(5).await;
    let want = ids(&["m0", "m1", "m2", "m3", "m4"]);
    for i in 0..3 {
        assert_eq!(g.message_ids(i), want, "node {i} holds m0..m4 in order");
        assert_eq!(
            g.raft(i).relay().log_len("s").unwrap(),
            5,
            "node {i} holds exactly 5 entries (no double-apply)"
        );
        assert_eq!(
            g.raft(i).applied_index(),
            g.raft(leader).applied_index(),
            "node {i} applied_index equals the leader's"
        );
    }

    let (_, dup) = g.raft(follower).publish(&cmd("m1")).await.unwrap();
    assert!(
        dup.expect("outcome").deduped,
        "republish of m1 after restart dedupes"
    );
    assert_eq!(
        g.raft(leader).relay().log_len("s").unwrap(),
        5,
        "deduped republish adds no entry: still 5"
    );
    g.shutdown().await;
}

/// Test 2, leader restart: the old leader rejoins as a follower, converges,
/// and a publish sent to it is applied on all three.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_voter_leader_restart_rejoins_as_follower() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut g = Group::prepare(3).await;
    g.start_all();
    let old = g.wait_single_leader().await;
    let (_, out) = g.raft(old).publish(&cmd("a")).await.unwrap();
    assert!(!out.expect("leader claims outcome").deduped, "publish a");
    g.wait_converged(1).await;

    g.kill(old).await;
    let new = g.wait_single_leader().await;
    assert_ne!(new, old, "a new leader replaces the killed one");
    let (_, out) = g.raft(new).publish(&cmd("b")).await.unwrap();
    assert!(!out.expect("leader claims outcome").deduped, "publish b");
    g.wait_converged(2).await;

    g.start_node(old);
    let only = g.wait_single_leader().await;
    assert_ne!(only, old, "restarted node rejoins as a follower");
    g.wait_converged(2).await;
    for i in 0..3 {
        assert_eq!(
            g.message_ids(i),
            ids(&["a", "b"]),
            "node {i} holds a,b after leader restart"
        );
    }

    let (_, out) = g.raft(old).publish(&cmd("c")).await.unwrap();
    assert!(
        !out.expect("restarted node claims outcome").deduped,
        "publish c via restarted node"
    );
    g.wait_converged(3).await;
    for i in 0..3 {
        assert_eq!(
            g.message_ids(i),
            ids(&["a", "b", "c"]),
            "node {i} holds a,b,c after publish via restarted node"
        );
    }
    g.shutdown().await;
}

/// Test 3, full-group restart: all three stop and start on their own dirs;
/// a leader is elected within 8 s, history is intact, and a new publish is
/// entry 4 on all three.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_voter_full_group_restart_keeps_history_and_elects() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut g = Group::prepare(3).await;
    g.start_all();
    let leader = g.wait_single_leader().await;
    for m in ["a", "b", "c"] {
        let (_, out) = g.raft(leader).publish(&cmd(m)).await.unwrap();
        assert!(!out.expect("leader claims outcome").deduped, "publish {m}");
    }
    g.wait_converged(3).await;

    for i in 0..3 {
        g.kill(i).await;
    }
    g.start_all();
    let leader = g.wait_single_leader().await;
    g.wait_converged(3).await;
    for i in 0..3 {
        assert_eq!(
            g.message_ids(i),
            ids(&["a", "b", "c"]),
            "node {i} holds a,b,c after full restart"
        );
    }

    let (_, out) = g.raft(leader).publish(&cmd("d")).await.unwrap();
    assert!(!out.expect("leader claims outcome").deduped, "publish d");
    g.wait_converged(4).await;
    for i in 0..3 {
        assert_eq!(
            g.message_ids(i),
            ids(&["a", "b", "c", "d"]),
            "node {i} holds a,b,c,d (d is entry 4)"
        );
    }
    g.shutdown().await;
}

// HANDWRITE-END
