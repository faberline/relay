//! Shared in-process multi-node cluster test harness for `it` integration tests.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use raft_runtime::Membership;
use relay::{PubCommand, Relay, RelayCoreConfig, RelayRaft};

pub static CLUSTER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn cmd(id: &str) -> PubCommand {
    PubCommand {
        subject: "s".to_string(),
        message_id: id.to_string(),
        payload: serde_json::json!({ "m": id }),
        headers: Default::default(),
        priority: relay::DEFAULT_PRIORITY,
        not_before: None,
        appended_at: chrono::Utc::now(),
    }
}

pub struct Node {
    pub raft: Arc<RelayRaft>,
    pub serve: tokio::task::JoinHandle<()>,
    pub shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

/// An in-process group: every node's listener + URL is bound up-front so peer
/// maps are complete, but nodes start individually (the snapshot test starts
/// one late).
pub struct Cluster {
    pub urls: HashMap<u64, String>,
    pub listeners: Vec<Option<tokio::net::TcpListener>>,
    pub membership: Membership,
    pub nodes: Vec<Option<Node>>,
    pub _dirs: Vec<tempfile::TempDir>,
    pub client: reqwest::Client,
}

impl Cluster {
    pub async fn prepare(n: u64) -> Cluster {
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
            _dirs: (0..n).map(|_| tempfile::tempdir().unwrap()).collect(),
            client: reqwest::Client::builder()
                .http2_prior_knowledge()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
        }
    }

    /// Spawn node `id` (durable engine + raft state in its node directory) and
    /// serve its peer router on the pre-bound listener.
    pub fn start_node(&mut self, id: usize, snapshot_every: u64) {
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

    pub fn start_all(&mut self, snapshot_every: u64) {
        for id in 0..self.nodes.len() {
            self.start_node(id, snapshot_every);
        }
    }

    pub fn leave_listener_unbound_until_start(&mut self, id: usize) {
        drop(self.listeners[id].take());
    }

    pub fn raft(&self, id: usize) -> &Arc<RelayRaft> {
        &self.nodes[id].as_ref().expect("node running").raft
    }

    /// Abort the node's serve loop and drop its host (tick/pump abort on drop).
    pub async fn kill(&mut self, id: usize) {
        if let Some(mut n) = self.nodes[id].take() {
            n.shutdown.take();
            n.serve.abort();
            let _ = n.serve.await;
            n.raft.shutdown().await.unwrap();
        }
    }

    pub async fn shutdown(mut self) {
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

    pub fn live(&self) -> impl Iterator<Item = (usize, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| n.as_ref().map(|n| (i, n)))
    }

    pub async fn leader(&self) -> Option<usize> {
        for (i, n) in self.live() {
            if n.raft.is_leader().await {
                return Some(i);
            }
        }
        None
    }

    pub async fn wait_leader(&self) -> usize {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(i) = self.leader().await {
                return i;
            }
            assert!(Instant::now() < deadline, "no leader elected");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait until every LIVE node's engine holds at least `want` entries.
    pub async fn wait_converged(&self, want: u64) {
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

    pub fn message_ids(&self, id: usize) -> Vec<String> {
        let r = self.raft(id).relay();
        let len = r.log_len("s").unwrap();
        (0..len)
            .filter_map(|seq| r.entry("s", 0, seq).unwrap().map(|e| e.message_id))
            .collect()
    }
}
