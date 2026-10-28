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

static CLUSTER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Node {
    raft: Arc<RelayRaft>,
    serve: tokio::task::JoinHandle<()>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

struct Cluster {
    urls: HashMap<u64, String>,
    listeners: Vec<Option<tokio::net::TcpListener>>,
    membership: Membership,
    nodes: Vec<Option<Node>>,
    dirs: Vec<tempfile::TempDir>,
}

impl Cluster {
    async fn prepare(n: u64) -> Cluster {
        let mut listeners = Vec::new();
        let mut urls = HashMap::new();
        for id in 0..n {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            urls.insert(id, format!("http://{}", l.local_addr().unwrap()));
            listeners.push(Some(l));
        }
        Cluster {
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

    fn start_node(&mut self, id: usize, snapshot_every: u64) {
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
                RelayRaft::host_config(snapshot_every),
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
                let deadline = Instant::now() + Duration::from_secs(2);
                let listener = loop {
                    match std::net::TcpListener::bind(address) {
                        Ok(l) => break l,
                        Err(_e) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(e) => panic!("bind failed on {address}: {e}"),
                    }
                };
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

    fn start_all(&mut self, snapshot_every: u64) {
        for id in 0..self.nodes.len() {
            self.start_node(id, snapshot_every);
        }
    }

    fn raft(&self, id: usize) -> &Arc<RelayRaft> {
        &self.nodes[id].as_ref().expect("node running").raft
    }

    async fn kill(&mut self, id: usize) {
        if let Some(mut n) = self.nodes[id].take() {
            if let Some(shutdown) = n.shutdown.take() {
                let _ = shutdown.send(());
            }
            n.serve.abort();
            let _ = n.serve.await;
            n.raft.shutdown().await.unwrap();
        }
    }

    async fn shutdown(mut self) {
        for node in self.nodes.iter().flatten() {
            let _ = node.raft.shutdown().await;
        }
        for node in &mut self.nodes {
            if let Some(mut node) = node.take() {
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

    async fn leader(&self) -> Option<usize> {
        for (i, n) in self.live() {
            if n.raft.is_leader().await {
                return Some(i);
            }
        }
        None
    }

    async fn wait_leader(&self) -> usize {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(i) = self.leader().await {
                return i;
            }
            assert!(Instant::now() < deadline, "no leader elected");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn wait_converged(&self, want: u64) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let ok = self
                .live()
                .all(|(_, n)| n.raft.relay().log_len("s").unwrap() >= want);
            if ok {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "engines did not converge to {want}"
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

/// AC2 / Test 1 (follower restart): publish 3 messages, wait until all nodes hold
/// 3 entries, stop one follower, publish 2 more through the live pair, restart
/// the follower on the same dirs. It converges to 5 entries with message ids
/// in the same order as the other nodes, log_len is 5 on every node (no double-apply),
/// and its applied_index() equals the others'.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_node_follower_restart_rejoins_without_double_apply() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    c.start_all(1024);

    let leader = c.wait_leader().await;

    // publish 3 messages, wait until all nodes hold 3 entries
    for i in 0..3 {
        let (_, out) = c
            .raft(leader)
            .publish(&cmd(&format!("m{i}")))
            .await
            .unwrap();
        assert!(
            !out.unwrap_or_else(|| panic!("node {leader}: expected count 1 initial outcome"))
                .deduped,
            "node {leader}: expected count 1 initial publish outcome not deduped"
        );
    }
    c.wait_converged(3).await;
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            3,
            "node {i}: expected count 3 initial entries"
        );
    }

    // stop one follower
    let follower = c.live().map(|(i, _)| i).find(|i| *i != leader).unwrap();
    c.kill(follower).await;

    // publish 2 more through the live pair
    for i in 3..5 {
        let (_, out) = c
            .raft(leader)
            .publish(&cmd(&format!("m{i}")))
            .await
            .unwrap();
        assert!(
            !out.unwrap_or_else(|| panic!("node {leader}: expected count 1 live pair outcome"))
                .deduped,
            "node {leader}: expected count 1 live pair publish outcome not deduped"
        );
    }
    c.wait_converged(5).await;
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            5,
            "node {i}: expected count 5 entries on live pair"
        );
    }

    // restart the follower on the same dirs
    c.start_node(follower, 1024);

    // It converges to 5 entries with message ids in the same order as the other nodes,
    // log_len is 5 on every node (no double-apply), and its applied_index() equals the others'.
    c.wait_converged(5).await;
    let expected_ids = c.message_ids(leader);
    assert_eq!(
        expected_ids.len(),
        5,
        "node {leader}: expected count 5 message ids"
    );
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            5,
            "node {i}: expected count 5 entries with no double-apply"
        );
        assert_eq!(
            c.message_ids(i),
            expected_ids,
            "node {i}: expected count 5 message ids in order"
        );
    }

    let leader_applied = c.raft(leader).applied_index();
    assert_eq!(
        c.raft(follower).applied_index(),
        leader_applied,
        "node {follower}: expected count {leader_applied} matching leader {leader} applied_index"
    );

    // Reuse the existing publish-by-id idempotency: republishing an old message_id
    // after the restart returns deduped and does not add an entry.
    let (_, dup) = c.raft(follower).publish(&cmd("m1")).await.unwrap();
    let dup_out = dup.unwrap_or_else(|| panic!("node {follower}: expected count 1 dedup outcome"));
    assert!(
        dup_out.deduped,
        "node {follower}: expected count 1 dedup outcome on duplicate publish"
    );
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            5,
            "node {i}: expected count 5 entries after duplicate publish"
        );
    }

    c.shutdown().await;
}

/// AC2 / Test 2 (leader restart): stop the leader, wait for a new leader among
/// the other two, publish 1 message, restart the old leader on the same dirs.
/// It rejoins as a follower (no second leader while the new one is live: exactly
/// one is_leader() across the three within the deadline), converges to the same
/// entries, and a publish sent to the restarted node is forwarded or accepted
/// and applied on all three.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_node_leader_restart_rejoins_as_follower_and_converges() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    c.start_all(1024);

    let old_leader = c.wait_leader().await;

    // Publish 2 initial messages through old_leader so there is state before failover
    for i in 0..2 {
        let (_, out) = c
            .raft(old_leader)
            .publish(&cmd(&format!("m{i}")))
            .await
            .unwrap();
        assert!(
            !out.unwrap_or_else(|| panic!("node {old_leader}: expected count 1 initial outcome"))
                .deduped,
            "node {old_leader}: expected count 1 initial publish outcome not deduped"
        );
    }
    c.wait_converged(2).await;
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            2,
            "node {i}: expected count 2 initial entries"
        );
    }

    // stop the leader
    c.kill(old_leader).await;

    // wait for a new leader among the other two
    let new_leader = c.wait_leader().await;
    assert_ne!(
        new_leader, old_leader,
        "node {new_leader}: expected count 1 new leader replacing node {old_leader}"
    );

    // publish 1 message
    let (_, out) = c.raft(new_leader).publish(&cmd("m2")).await.unwrap();
    assert!(
        !out.unwrap_or_else(|| panic!("node {new_leader}: expected count 1 failover outcome"))
            .deduped,
        "node {new_leader}: expected count 1 failover publish outcome not deduped"
    );
    c.wait_converged(3).await;
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            3,
            "node {i}: expected count 3 entries on surviving live pair"
        );
    }

    // restart the old leader on the same dirs
    c.start_node(old_leader, 1024);

    // It rejoins as a follower (no second leader while the new one is live: exactly
    // one is_leader() across the three within the deadline)
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut leaders = 0;
    while Instant::now() < deadline {
        leaders = 0;
        for (_, n) in c.live() {
            if n.raft.is_leader().await {
                leaders += 1;
            }
        }
        if leaders == 1 && !c.raft(old_leader).is_leader().await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        leaders, 1,
        "node {old_leader}: expected count 1 leader across the group after restart"
    );
    assert!(
        !c.raft(old_leader).is_leader().await,
        "node {old_leader}: expected count 0 is_leader when rejoining as follower"
    );

    // converges to the same entries
    c.wait_converged(3).await;
    let expected_ids = c.message_ids(new_leader);
    assert_eq!(
        expected_ids.len(),
        3,
        "node {new_leader}: expected count 3 message ids"
    );
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            3,
            "node {i}: expected count 3 entries after convergence"
        );
        assert_eq!(
            c.message_ids(i),
            expected_ids,
            "node {i}: expected count 3 message ids in order"
        );
    }

    // and a publish sent to the restarted node is forwarded or accepted and applied on all three
    let (_, fw) = c.raft(old_leader).publish(&cmd("m3")).await.unwrap();
    let fw_out =
        fw.unwrap_or_else(|| panic!("node {old_leader}: expected count 1 forwarded outcome"));
    assert!(
        !fw_out.deduped,
        "node {old_leader}: expected count 1 forwarded publish outcome not deduped"
    );
    c.wait_converged(4).await;
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            4,
            "node {i}: expected count 4 entries after forwarded publish"
        );
        assert!(
            c.message_ids(i).contains(&"m3".to_string()),
            "node {i}: expected count 1 occurrence of message m3"
        );
    }

    // Reuse the existing publish-by-id idempotency: republishing an old message_id
    // after the restart returns deduped and does not add an entry
    let (_, dup) = c.raft(old_leader).publish(&cmd("m2")).await.unwrap();
    let dup_out =
        dup.unwrap_or_else(|| panic!("node {old_leader}: expected count 1 dedup outcome"));
    assert!(
        dup_out.deduped,
        "node {old_leader}: expected count 1 dedup outcome on duplicate publish"
    );
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            4,
            "node {i}: expected count 4 entries after duplicate publish"
        );
    }

    c.shutdown().await;
}

/// AC2 / Test 3 (full-group restart): publish 3 messages, stop all three nodes,
/// start all three on the same dirs. A leader is elected within the 8 s deadline
/// used by wait_leader, all three still hold exactly the 3 messages in order,
/// and a new publish commits on all three as entry 4 (no lost vote or term that
/// blocks or splits the election).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_node_full_group_restart_recovers_state_and_elects_leader() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3).await;
    c.start_all(1024);

    let leader = c.wait_leader().await;

    // publish 3 messages
    for i in 0..3 {
        let (_, out) = c
            .raft(leader)
            .publish(&cmd(&format!("m{i}")))
            .await
            .unwrap();
        assert!(
            !out.unwrap_or_else(|| panic!("node {leader}: expected count 1 initial outcome"))
                .deduped,
            "node {leader}: expected count 1 initial publish outcome not deduped"
        );
    }
    c.wait_converged(3).await;
    for (i, _) in c.live() {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            3,
            "node {i}: expected count 3 entries before full restart"
        );
    }

    // stop all three nodes
    for i in 0..3 {
        c.kill(i).await;
    }

    // start all three on the same dirs
    for i in 0..3 {
        c.start_node(i, 1024);
    }

    // A leader is elected within the 8 s deadline used by wait_leader
    let new_leader = c.wait_leader().await;
    assert!(
        new_leader < 3,
        "node {new_leader}: expected count 1 leader elected within deadline"
    );

    // all three still hold exactly the 3 messages in order
    c.wait_converged(3).await;
    for i in 0..3 {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            3,
            "node {i}: expected count 3 recovered entries"
        );
        let ids = c.message_ids(i);
        assert_eq!(
            ids,
            vec!["m0", "m1", "m2"],
            "node {i}: expected count 3 messages in order"
        );
    }

    // and a new publish commits on all three as entry 4 (no lost vote or term that blocks or splits the election)
    let (idx, out) = c.raft(new_leader).publish(&cmd("m3")).await.unwrap();
    assert_eq!(
        idx, 4,
        "node {new_leader}: expected count 4 at raft index 4"
    );
    let out = out.unwrap_or_else(|| panic!("node {new_leader}: expected count 1 publish outcome"));
    assert!(
        !out.deduped,
        "node {new_leader}: expected count 1 outcome not deduped"
    );

    c.wait_converged(4).await;
    for i in 0..3 {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            4,
            "node {i}: expected count 4 committed entries after restart"
        );
        let ids = c.message_ids(i);
        assert_eq!(
            ids,
            vec!["m0", "m1", "m2", "m3"],
            "node {i}: expected count 4 messages in recovered order"
        );
    }

    // Reuse the existing publish-by-id idempotency: republishing an old message_id
    // after the restart returns deduped and does not add an entry
    let (_, dup) = c.raft(new_leader).publish(&cmd("m1")).await.unwrap();
    let dup_out =
        dup.unwrap_or_else(|| panic!("node {new_leader}: expected count 1 dedup outcome"));
    assert!(
        dup_out.deduped,
        "node {new_leader}: expected count 1 deduped outcome on duplicate publish"
    );
    for i in 0..3 {
        assert_eq!(
            c.raft(i).relay().log_len("s").unwrap(),
            4,
            "node {i}: expected count 4 entries after duplicate publish"
        );
    }

    println!("claim observed: durable raft hard state restore");
    c.shutdown().await;
}
// HANDWRITE-END
