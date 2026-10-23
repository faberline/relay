use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use raft_runtime::Membership;
use relay::{PubCommand, Relay, RelayCoreConfig, RelayRaft};

static CLUSTER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    _dirs: Vec<tempfile::TempDir>,
    #[allow(dead_code)]
    client: reqwest::Client,
}

impl Cluster {
    async fn prepare(voters: u64, learners: u64) -> Cluster {
        let total = voters + learners;
        let mut listeners = Vec::new();
        let mut urls = HashMap::new();
        for id in 0..total {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            urls.insert(id, format!("http://{}", l.local_addr().unwrap()));
            listeners.push(Some(l));
        }
        Cluster {
            urls,
            listeners,
            membership: Membership {
                voters: (0..voters).collect(),
                learners: (voters..total).collect(),
            },
            nodes: (0..total).map(|_| None).collect(),
            _dirs: (0..total).map(|_| tempfile::tempdir().unwrap()).collect(),
            client: reqwest::Client::builder()
                .http2_prior_knowledge()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
        }
    }

    fn leave_listener_unbound_until_start(&mut self, id: usize) {
        drop(self.listeners[id].take());
    }

    fn start_node(&mut self, id: usize, snapshot_every: u64) {
        let engine = Arc::new(Relay::new(RelayCoreConfig {
            data_dir: self._dirs[id]
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
                &self._dirs[id].path().join("raft"),
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

    fn raft(&self, id: usize) -> &Arc<RelayRaft> {
        &self.nodes[id].as_ref().expect("node running").raft
    }

    async fn kill(&mut self, id: usize) {
        if let Some(mut n) = self.nodes[id].take() {
            n.shutdown.take();
            n.serve.abort();
            let _ = n.serve.await;
            n.raft.shutdown().await.unwrap();
        }
    }

    async fn shutdown(mut self) {
        for node in self.nodes.iter().flatten() {
            node.raft.shutdown().await.unwrap();
        }
        for node in &mut self.nodes {
            if let Some(mut node) = node.take() {
                if let Some(shutdown) = node.shutdown.take() {
                    let _ = shutdown.send(());
                }
                let _ = node.serve.await;
            }
        }
        drop(self.client);
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
            assert!(
                Instant::now() < deadline,
                "node 0 seq 0: no leader elected within deadline"
            );
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
                "node 0 seq {want}: engines did not converge to {want}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordered_log_queue_and_raft_feature_breadth() {
    let _guard = CLUSTER_TEST_LOCK.lock().await;
    let mut c = Cluster::prepare(3, 1).await;
    // Leave learner unbound until it is added to the cluster.
    c.leave_listener_unbound_until_start(3);
    c.start_node(0, 1024);
    c.start_node(1, 1024);
    c.start_node(2, 1024);

    let leader = c.wait_leader().await;
    let mut leaders = 0;
    for (_, n) in c.live() {
        if n.raft.is_leader().await {
            leaders += 1;
        }
    }
    assert_eq!(
        leaders, 1,
        "node {leader} seq 0: exactly one leader elected"
    );

    // 1. Publish at least 8 messages to one stream through different live nodes.
    for i in 0..8u64 {
        let node_id = (i as usize) % 3;
        let (_, outcome) = c
            .raft(node_id)
            .publish(&cmd(&format!("m{i}")))
            .await
            .unwrap();
        let outcome = outcome.expect("publish outcome");
        assert!(
            !outcome.deduped,
            "node {node_id} seq {i}: published message m{i} was not deduped"
        );
    }
    c.wait_converged(8).await;

    // 2. Every live node returns the same ids in the same order with strictly increasing seq.
    for (node_id, _) in c.live() {
        for seq in 0..8u64 {
            let entry = c
                .raft(node_id)
                .relay()
                .entry("s", 0, seq)
                .unwrap()
                .expect("log entry present");
            assert_eq!(
                entry.seq, seq,
                "node {node_id} seq {seq}: entry sequence is strictly increasing and matches"
            );
            assert_eq!(
                entry.message_id,
                format!("m{seq}"),
                "node {node_id} seq {seq}: message_id matches expected m{seq}"
            );
        }
    }

    // 3. Leases taken on the replicas drain the stream in seq order per replica;
    // acks advance committed_offset to the same value on every live node within a bounded deadline.
    for seq in 0..8u64 {
        let replica = (seq as usize) % 3;
        let lease = c
            .raft(replica)
            .lease("s".into(), format!("worker-{replica}"), Utc::now())
            .await
            .unwrap()
            .expect("lease granted");
        assert_eq!(
            lease.seq, seq,
            "node {replica} seq {seq}: lease granted in strict seq order"
        );
        assert_eq!(
            lease.executor_node, replica as u64,
            "node {replica} seq {seq}: executor node matches replica"
        );

        let (accepted, committed) = c
            .raft(replica)
            .ack("s".into(), lease.lease_id, lease.epoch, Utc::now())
            .await
            .unwrap();
        assert!(
            accepted,
            "node {replica} seq {seq}: ack was accepted by executor"
        );
        let watermark = committed.expect("committed offset watermark");
        assert_eq!(
            watermark.committed_seq, seq,
            "node {replica} seq {seq}: committed_seq watermark advanced to {seq}"
        );
    }

    let ack_converged_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let ok = c.live().all(|(_, n)| {
            n.raft
                .relay()
                .committed_offset("s")
                .unwrap()
                .is_some_and(|offset| offset.committed_seq == 7)
        });
        if ok {
            break;
        }
        assert!(
            Instant::now() < ack_converged_deadline,
            "node 0 seq 7: committed_offset did not converge to seq 7 across all live nodes within deadline"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // 4. Kill the leader, wait for a new leader, publish more messages, and check that ordering
    // and offsets still converge, and that no message acked before the kill is leased again.
    let old_leader = c.wait_leader().await;
    c.kill(old_leader).await;

    let new_leader = c.wait_leader().await;
    assert_ne!(
        new_leader, old_leader,
        "node {new_leader} seq 8: new leader differs from killed leader {old_leader}"
    );

    for i in 8..12u64 {
        let (_, outcome) = c
            .raft(new_leader)
            .publish(&cmd(&format!("m{i}")))
            .await
            .unwrap();
        let outcome = outcome.expect("failover publish outcome");
        assert!(
            !outcome.deduped,
            "node {new_leader} seq {i}: post-failover publish m{i} was not deduped"
        );
    }
    c.wait_converged(12).await;

    // Verify that no message acked before the kill (seq 0..7) is leased again.
    let failover_lease = c
        .raft(new_leader)
        .lease("s".into(), "worker-post-failover".into(), Utc::now())
        .await
        .unwrap()
        .expect("lease granted post failover");
    assert_eq!(
        failover_lease.seq,
        8,
        "node {new_leader} seq 8: first lease post failover is seq 8; acked seqs 0..7 are not leased again"
    );
    let (accepted, committed) = c
        .raft(new_leader)
        .ack(
            "s".into(),
            failover_lease.lease_id,
            failover_lease.epoch,
            Utc::now(),
        )
        .await
        .unwrap();
    assert!(
        accepted,
        "node {new_leader} seq 8: post-failover ack for seq 8 accepted"
    );
    assert_eq!(
        committed.expect("watermark").committed_seq,
        8,
        "node {new_leader} seq 8: committed watermark advanced to 8"
    );

    let live_survivors: Vec<usize> = c.live().map(|(id, _)| id).collect();
    for seq in 9..12u64 {
        let replica = live_survivors[(seq as usize) % live_survivors.len()];
        let lease = c
            .raft(replica)
            .lease("s".into(), format!("worker-{replica}"), Utc::now())
            .await
            .unwrap()
            .expect("post-failover lease granted");
        assert_eq!(
            lease.seq, seq,
            "node {replica} seq {seq}: post-failover lease seq matches {seq}"
        );
        let (accepted, committed) = c
            .raft(replica)
            .ack("s".into(), lease.lease_id, lease.epoch, Utc::now())
            .await
            .unwrap();
        assert!(
            accepted,
            "node {replica} seq {seq}: post-failover ack for seq {seq} accepted"
        );
        assert_eq!(
            committed.expect("watermark").committed_seq,
            seq,
            "node {replica} seq {seq}: committed watermark advanced to {seq}"
        );
    }

    let failover_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let ok = c.live().all(|(_, n)| {
            n.raft
                .relay()
                .committed_offset("s")
                .unwrap()
                .is_some_and(|offset| offset.committed_seq == 11)
        });
        if ok {
            break;
        }
        assert!(
            Instant::now() < failover_deadline,
            "node {new_leader} seq 11: offsets did not converge to seq 11 after failover"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    for (node_id, _) in c.live() {
        for seq in 0..12u64 {
            let entry = c
                .raft(node_id)
                .relay()
                .entry("s", 0, seq)
                .unwrap()
                .expect("post-failover log entry present");
            assert_eq!(
                entry.seq, seq,
                "node {node_id} seq {seq}: post-failover entry seq is strictly increasing"
            );
            assert_eq!(
                entry.message_id,
                format!("m{seq}"),
                "node {node_id} seq {seq}: post-failover entry message_id matches expected m{seq}"
            );
        }
    }

    // 5. Add a learner and check it replicates the log in order but does not change the leader election outcome.
    c.start_node(3, 1024);

    let learner_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if c.raft(3).relay().log_len("s").unwrap() >= 12 {
            break;
        }
        assert!(
            Instant::now() < learner_deadline,
            "node 3 seq 12: learner did not converge to 12 entries within deadline"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    for seq in 0..12u64 {
        let entry = c
            .raft(3)
            .relay()
            .entry("s", 0, seq)
            .unwrap()
            .expect("learner entry present");
        assert_eq!(
            entry.seq, seq,
            "node 3 seq {seq}: learner log seq is strictly increasing and matches"
        );
        assert_eq!(
            entry.message_id,
            format!("m{seq}"),
            "node 3 seq {seq}: learner message_id matches expected m{seq}"
        );
    }

    assert!(
        !c.raft(3).is_leader().await,
        "node 3 seq 0: learner is not leader and never leads"
    );
    let current_leader = c.wait_leader().await;
    assert_eq!(
        current_leader,
        new_leader,
        "node 3 seq 0: adding learner did not change leader election outcome (leader remains node {new_leader})"
    );

    println!("claim observed: ordered log queue and raft feature breadth");
    c.shutdown().await;
}
