// HANDWRITE-BEGIN gap="missing-generator:unit-test:a8a111d3" tracker="pending-tracker" reason="Live-node integration over the http2_transport.rs harness (service_http::serve on 127.0.0.1:0): /admin/backup returns parseable EngineSnapshot bytes carrying a published un-acked message; the artifact round-trips through load_snapshot_bytes on a FRESH engine and the message leases back with the original payload (idempotent merge re-load asserted); with auth required the endpoint 401s tokenless, 403s a non-admin token, 200s an admin-on-* token; cfg(feature = backup): relay::replication::backup::run_backup ships the snapshot to a file:// sink (BackupRunResult + artifact on disk) and prunes by retention."
//! Backup surface over a LIVE node (WI #1209): `GET /admin/backup` returns
//! the exact `RelayStateMachine::snapshot` bytes (`EngineSnapshot` =
//! `dump_live` + applied index), the artifact round-trips through the
//! exact `load_live` state replacement on a fresh engine, the endpoint is admin-guarded when
//! auth is required, and — with `--features backup` — `relay::replication::backup` ships
//! the bytes to a `core/service-backup` `file://` sink with age retention.
//!
//! Harness: the shared service shell's serve loop on an ephemeral loopback
//! port (the `http2_transport.rs` pattern), driven with plain reqwest.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use serde_json::json;

use relay::queue::server::{router, AppState};
use relay::queue::server_config::RelayServerConfig;
use relay::{Relay, RelayCoreConfig};

async fn start_server_with_state(state: AppState) -> SocketAddr {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Tests never signal shutdown; the loop lives for the test process.
    tokio::spawn(service_http::serve(
        listener,
        app,
        std::future::pending::<()>(),
    ));
    addr
}

async fn start_server() -> (SocketAddr, AppState) {
    let state = AppState::new(RelayServerConfig::ephemeral(), 8 * 1024 * 1024);
    let addr = start_server_with_state(state.clone()).await;
    (addr, state)
}

/// R2 / AC2 (endpoint): `GET /admin/backup` on a live node returns the exact
/// bytes `RelayStateMachine::snapshot` would produce — the shared
/// `raft::snapshot_bytes` serialization (single-node: applied floor 0) — and
/// they parse as an `EngineSnapshot` carrying the published un-acked message.
#[tokio::test]
async fn admin_backup_returns_the_state_machine_snapshot_bytes() {
    let (addr, state) = start_server().await;
    state
        .relay_handle()
        .publish(
            "jobs",
            "m-1",
            json!({"task": "t"}),
            BTreeMap::new(),
            Utc::now(),
        )
        .unwrap();

    let resp = reqwest::get(format!("http://{addr}/admin/backup"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();

    // ONE format: the endpoint bytes equal the state machine's serialization.
    let expected = relay::snapshot_bytes(&state.relay_handle(), 0).unwrap();
    assert_eq!(
        &bytes[..],
        &expected[..],
        "endpoint bytes == RelayStateMachine::snapshot bytes"
    );

    let snap: relay::EngineSnapshot = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(snap.up_to, 0, "no raft attached: applied floor is 0");
    assert_eq!(snap.subjects.len(), 1);
    assert_eq!(snap.subjects[0].subject, "jobs");
    assert_eq!(snap.subjects[0].entries.len(), 1);
    assert_eq!(snap.subjects[0].entries[0].message_id, "m-1");
}

/// R2 / AC2 (restore): a backup artifact written through the service-backup
/// local sink round-trips: `load_snapshot_bytes` replaces a FRESH engine, a
/// consumer leases the message back with the original payload, and loading the
/// same artifact twice remains idempotent.
#[tokio::test]
async fn backup_artifact_round_trips_through_load_live_on_a_fresh_engine() {
    let (addr, state) = start_server().await;
    state
        .relay_handle()
        .publish("jobs", "m-1", json!({"n": 42}), BTreeMap::new(), Utc::now())
        .unwrap();

    let bytes = reqwest::get(format!("http://{addr}/admin/backup"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    // Persist the artifact exactly the way a backup run does (local sink).
    let dir = tempfile::tempdir().unwrap();
    let sink = service_backup::LocalFsSink::new(dir.path(), "relay").unwrap();
    let key = service_backup::BackupSink::put(&sink, std::time::SystemTime::now(), &bytes).unwrap();
    let artifact = std::fs::read(dir.path().join(&key)).unwrap();

    // Fresh node: restore = exact state replacement.
    let fresh = Relay::new(RelayCoreConfig::in_memory());
    let up_to = relay::load_snapshot_bytes(&fresh, &artifact).unwrap();
    assert_eq!(up_to, 0);
    // Idempotent: a second load of the same artifact dedupes, never appends.
    relay::load_snapshot_bytes(&fresh, &artifact).unwrap();

    let now = Utc::now();
    let lease = fresh
        .lease("jobs", "w-1", now)
        .unwrap()
        .expect("restored message leases back");
    let entry = fresh
        .entry(&lease.subject, lease.shard, lease.seq)
        .unwrap()
        .expect("leased entry body");
    assert_eq!(entry.message_id, "m-1");
    assert_eq!(entry.payload, json!({"n": 42}));
    // Exactly one message exists after the double load.
    assert!(
        fresh.lease("jobs", "w-2", now).unwrap().is_none(),
        "double load must not duplicate the message"
    );
}

#[test]
fn restore_preserves_truncated_sequence_space_and_committed_watermark() {
    let source = Relay::new(RelayCoreConfig::in_memory());
    let now = Utc::now();
    for i in 0..5 {
        source
            .publish(
                "jobs",
                &format!("m-{i}"),
                json!({"n": i}),
                BTreeMap::new(),
                now,
            )
            .unwrap();
    }
    for i in 0..3 {
        let lease = source
            .lease("jobs", "worker", now)
            .unwrap()
            .expect("source lease");
        assert_eq!(lease.seq, i);
        assert!(source
            .ack("jobs", &lease.lease_id, Some(lease.epoch))
            .unwrap());
    }

    let artifact = relay::snapshot_bytes(&source, 17).unwrap();
    let restored = Relay::new(RelayCoreConfig::in_memory());
    assert_eq!(
        relay::load_snapshot_bytes(&restored, &artifact).unwrap(),
        17
    );
    assert!(restored.entry("jobs", 0, 0).unwrap().is_none());
    assert_eq!(restored.entry("jobs", 0, 3).unwrap().unwrap().seq, 3);
    assert_eq!(
        restored
            .committed_offset("jobs")
            .unwrap()
            .expect("watermark")
            .committed_seq,
        2
    );
    let appended = restored
        .publish("jobs", "m-5", json!({"n": 5}), BTreeMap::new(), now)
        .unwrap();
    assert_eq!(appended.seq, 5, "snapshot restore must not rewind seq");
}

/// R2 (guard): with auth required, `GET /admin/backup` rejects a missing
/// token (401) and a non-admin token (403), and serves an admin-on-`*` token
/// (200); probes stay tokenless either way (lumen's guard shape).
#[tokio::test]
async fn admin_backup_requires_admin_when_auth_required() {
    let tokens = json!({
        "admin-token": { "subject": "ops", "roles": { "*": "admin" } },
        "reader-token": { "subject": "worker", "roles": { "*": "read" } },
    })
    .to_string();
    let auth = relay::access::auth::AuthConfig::resolve("required", None, Some(&tokens)).unwrap();
    let state = AppState::with_auth(RelayServerConfig::ephemeral(), auth, 8 * 1024 * 1024);
    let addr = start_server_with_state(state).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/admin/backup");
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        401,
        "tokenless is rejected"
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("reader-token")
            .send()
            .await
            .unwrap()
            .status(),
        403,
        "a cluster-wide admin op needs admin on `*`"
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("admin-token")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // Probes stay tokenless.
    assert_eq!(
        client
            .get(format!("http://{addr}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

/// R2 / AC2 (verb, feature `backup`): the library path `relay backup` drives —
/// fetch the snapshot over HTTP, ship it to a `file://` destination sink, and
/// apply age retention (the pre-aged object is pruned, the fresh artifact
/// survives and carries the message).
#[cfg(feature = "backup")]
#[tokio::test]
async fn run_backup_ships_snapshot_to_local_sink() {
    use service_backup::{BackupDestination, RetentionPolicy};

    let (addr, state) = start_server().await;
    state
        .relay_handle()
        .publish(
            "jobs",
            "m-1",
            json!({"task": "t"}),
            BTreeMap::new(),
            Utc::now(),
        )
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    // Pre-age an object so the retention pass has something to prune.
    std::fs::write(dir.path().join("relay-0.json"), b"old").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let dest = BackupDestination::from_uri(&format!("file://{}", dir.path().display())).unwrap();
    let result = relay::replication::backup::run_backup(
        &format!("http://{addr}"),
        None,
        &dest,
        &RetentionPolicy::max_age_seconds(1),
    )
    .await
    .unwrap();

    assert!(result.object.bytes > 0);
    let artifact_path = dir.path().join(&result.object.key);
    assert!(artifact_path.exists(), "artifact written to the sink");
    assert_eq!(result.pruned, 1, "the pre-aged object is pruned");

    // The artifact is the same EngineSnapshot the endpoint serves.
    let artifact = std::fs::read(&artifact_path).unwrap();
    let snap: relay::EngineSnapshot = serde_json::from_slice(&artifact).unwrap();
    assert_eq!(snap.subjects[0].entries[0].message_id, "m-1");
}

/// R1 / AC3 (WI #17): `GET /admin/backup` on a node under concurrent publish,
/// lease, and ack traffic returns a snapshot that restores to a self-consistent
/// engine: the restored state equals a real prefix of writes (committed_seq
/// never rewinds, matches a prefix of writer acks, un-acked entries are
/// leasable with original payloads, and fresh publish assigns next seq).
#[tokio::test]
async fn concurrent_writes_produce_consistent_snapshots_during_backup() {
    let topic = "jobs";
    let state = AppState::new(RelayServerConfig::ephemeral(), 8 * 1024 * 1024);
    let addr = start_server_with_state(state.clone()).await;

    let stop = Arc::new(AtomicBool::new(false));
    let highest_acked = Arc::new(AtomicI64::new(-1));

    let stop_writer = Arc::clone(&stop);
    let highest_acked_writer = Arc::clone(&highest_acked);
    let relay = state.relay_handle();

    let writer_handle = tokio::spawn(async move {
        let mut pub_seq = 0u64;
        while !stop_writer.load(Ordering::Relaxed) {
            let pub_res = relay
                .publish(
                    topic,
                    &format!("m-{pub_seq}"),
                    json!({"n": pub_seq}),
                    BTreeMap::new(),
                    Utc::now(),
                )
                .unwrap();
            let _ = pub_res.seq;
            pub_seq += 1;

            if pub_seq > 2 {
                if let Some(lease) = relay.lease(topic, "writer-worker", Utc::now()).unwrap() {
                    let seq = lease.seq;
                    let acked = relay
                        .ack(topic, &lease.lease_id, Some(lease.epoch))
                        .unwrap();
                    if acked {
                        highest_acked_writer.fetch_max(seq as i64, Ordering::SeqCst);
                    }
                }
            }

            tokio::task::yield_now().await;
        }
    });

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/admin/backup");
    let mut prev_committed_seq: Option<u64> = None;

    for artifact_idx in 0..20 {
        let resp = client.get(&url).send().await.unwrap();
        let curr_seq_marker = prev_committed_seq.unwrap_or(0);
        assert_eq!(
            resp.status(),
            200,
            "artifact {artifact_idx}, seq {curr_seq_marker}: backup fetch status 200"
        );
        let bytes = resp.bytes().await.unwrap();
        let max_acked_at_fetch = highest_acked.load(Ordering::SeqCst);

        let restored = Relay::new(RelayCoreConfig::in_memory());
        let up_to = relay::load_snapshot_bytes(&restored, &bytes).unwrap();
        assert_eq!(
            up_to, 0,
            "artifact {artifact_idx}, seq {curr_seq_marker}: applied index is 0"
        );

        let committed_opt = restored.committed_offset(topic).unwrap();
        let curr_committed_seq = committed_opt.as_ref().map(|c| c.committed_seq);

        if let Some(ref c) = committed_opt {
            assert!(
                (c.committed_seq as i64) <= max_acked_at_fetch,
                "artifact {artifact_idx}, seq {}: committed_seq {} is at most highest acked {}",
                c.committed_seq,
                c.committed_seq,
                max_acked_at_fetch
            );
        }

        if let (Some(prev), Some(curr)) = (prev_committed_seq, curr_committed_seq) {
            assert!(
                curr >= prev,
                "artifact {artifact_idx}, seq {curr}: committed_seq {curr} never decreases from previous {prev}"
            );
        } else if let (Some(prev), None) = (prev_committed_seq, curr_committed_seq) {
            panic!(
                "artifact {artifact_idx}, seq 0: committed_seq decreased from Some({prev}) to None"
            );
        }
        if curr_committed_seq.is_some() {
            prev_committed_seq = curr_committed_seq;
        }

        let snap: relay::EngineSnapshot = serde_json::from_slice(&bytes).unwrap();
        let subj = snap.subjects.iter().find(|s| s.subject == topic);

        let mut leased_seqs = Vec::new();
        while let Some(lease) = restored.lease(topic, "verifier", Utc::now()).unwrap() {
            let entry = restored
                .entry(topic, lease.shard, lease.seq)
                .unwrap()
                .expect("entry present");
            assert_eq!(
                entry.payload,
                json!({"n": lease.seq}),
                "artifact {artifact_idx}, seq {}: leased payload matches original",
                lease.seq
            );
            leased_seqs.push(lease.seq);
        }

        if let Some(s) = subj {
            let expected_unacked: Vec<u64> = s.entries.iter().map(|e| e.seq).collect();
            let marker_seq = curr_committed_seq.unwrap_or(0);
            assert_eq!(
                leased_seqs, expected_unacked,
                "artifact {artifact_idx}, seq {marker_seq}: un-acked entries leased exactly once"
            );

            if s.next_seq > 0 {
                let highest_restored_seq = s.next_seq - 1;
                let pub_res = restored
                    .publish(
                        topic,
                        &format!("probe-{artifact_idx}"),
                        json!({"probe": artifact_idx}),
                        BTreeMap::new(),
                        Utc::now(),
                    )
                    .unwrap();
                assert_eq!(
                    pub_res.seq,
                    highest_restored_seq + 1,
                    "artifact {artifact_idx}, seq {highest_restored_seq}: new publish seq == highest restored seq + 1"
                );
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    stop.store(true, Ordering::Relaxed);
    writer_handle.await.unwrap();

    let final_resp = client.get(&url).send().await.unwrap();
    let pre_marker = prev_committed_seq.unwrap_or(0);
    assert_eq!(
        final_resp.status(),
        200,
        "artifact 20, seq {pre_marker}: final backup fetch status 200"
    );
    let final_bytes = final_resp.bytes().await.unwrap();

    let final_restored = Relay::new(RelayCoreConfig::in_memory());
    relay::load_snapshot_bytes(&final_restored, &final_bytes).unwrap();

    let restored_final_committed = final_restored
        .committed_offset(topic)
        .unwrap()
        .map(|c| c.committed_seq);
    let live_final_committed = state
        .relay_handle()
        .committed_offset(topic)
        .unwrap()
        .map(|c| c.committed_seq);

    let final_seq_marker = restored_final_committed.unwrap_or(0);
    assert_eq!(
        restored_final_committed, live_final_committed,
        "artifact 20, seq {final_seq_marker}: final restored committed_seq matches live engine"
    );

    let mut final_restored_unacked = Vec::new();
    while let Some(lease) = final_restored
        .lease(topic, "final-verifier", Utc::now())
        .unwrap()
    {
        final_restored_unacked.push(lease.seq);
    }

    let live_dumps = state.relay_handle().dump_live().unwrap();
    let live_unacked: Vec<u64> = live_dumps
        .iter()
        .find(|s| s.subject == topic)
        .map(|s| s.entries.iter().map(|e| e.seq).collect())
        .unwrap_or_default();

    assert_eq!(
        final_restored_unacked, live_unacked,
        "artifact 20, seq {final_seq_marker}: final restored un-acked seqs match live engine"
    );
}
// HANDWRITE-END
