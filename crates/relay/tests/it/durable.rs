// SPEC-MANAGED: tech-design/logic/default-durable-engine-throughput-group-commit-fsync-publish-bat.md#unit-test
// HANDWRITE-BEGIN gap="missing-generator:unit-test:7e699a0c" tracker="pending-tracker" reason="Tests: default config is durable, publish_batch group commit + dedupe, log recovery on reopen, and committed-offset crash recovery."
//! Default-durable throughput (#129): durable-by-default config, group-commit
//! publish_batch with in-batch dedupe, durable-log recovery on reopen, and
//! committed-offset crash recovery (resume after the committed watermark).

use std::collections::BTreeMap;
use std::process::{Child, Command};
use std::time::Duration;

use chrono::Utc;
use serde_json::json;

use relay::queue::wire::{AckResponse, LeaseRequest, LeaseResponse};
use relay::{FsyncPolicy, Relay, RelayCoreConfig, DEFAULT_PRIORITY};

fn item(id: &str) -> (String, serde_json::Value, BTreeMap<String, String>, u8) {
    (
        id.to_string(),
        serde_json::json!({ "id": id }),
        BTreeMap::new(),
        DEFAULT_PRIORITY,
    )
}

fn disk_config(dir: &std::path::Path) -> RelayCoreConfig {
    RelayCoreConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..RelayCoreConfig::default()
    }
}

fn segmented_config(dir: &std::path::Path, segment_bytes: u64) -> RelayCoreConfig {
    let mut cfg = disk_config(dir);
    cfg.segment_bytes = segment_bytes;
    cfg.ram_ring_entries = 4; // force disk-backed segments
    cfg
}

fn count_subject_segments(dir: &std::path::Path, subject: &str) -> usize {
    let prefix = format!("{subject}__shard0__");
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with(&prefix) && n.ends_with(".ndjson")
        })
        .count()
}

// relay is durable + power-safe out of the box.
#[test]
fn default_config_is_durable_and_power_safe() {
    let cfg = RelayCoreConfig::default();
    assert!(!cfg.data_dir.is_empty(), "default persists to disk");
    assert_eq!(
        cfg.fsync,
        FsyncPolicy::Always,
        "default fsync is power-safe"
    );
}

// publish_batch returns one outcome per message (group commit) and dedupes a
// repeated id within the batch.
#[test]
fn publish_batch_group_commit_and_dedupe() {
    let dir = tempfile::tempdir().unwrap();
    let r = Relay::new(disk_config(dir.path()));
    let now = Utc::now();
    let outcomes = r
        .publish_batch(
            "q",
            vec![item("m0"), item("m1"), item("m1"), item("m2")],
            now,
        )
        .unwrap();
    assert_eq!(outcomes.len(), 4);
    assert_eq!((outcomes[0].seq, outcomes[0].deduped), (0, false));
    assert_eq!((outcomes[1].seq, outcomes[1].deduped), (1, false));
    assert_eq!(
        (outcomes[2].seq, outcomes[2].deduped),
        (1, true),
        "in-batch dup"
    );
    assert_eq!((outcomes[3].seq, outcomes[3].deduped), (2, false));
    assert_eq!(r.log_len("q").unwrap(), 3);
}

// The durable log survives reopening from the same data_dir.
#[test]
fn durable_log_recovers_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    {
        let r = Relay::new(disk_config(dir.path()));
        r.publish_batch("q", vec![item("m0"), item("m1"), item("m2")], now)
            .unwrap();
    }
    let r2 = Relay::new(disk_config(dir.path()));
    assert_eq!(r2.log_len("q").unwrap(), 3, "entries recovered from disk");
    // a re-published id is still deduped after recovery.
    let out = r2.publish_batch("q", vec![item("m0")], now).unwrap();
    assert!(out[0].deduped);
}

// Crash recovery: the committed offset is durable, so a reopened relay resumes
// AFTER the committed watermark instead of redelivering everything.
#[test]
fn committed_offset_survives_crash() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    {
        let r = Relay::new(disk_config(dir.path()));
        r.publish_batch("q", (0..5).map(|i| item(&format!("m{i}"))).collect(), now)
            .unwrap();
        // lease + ack the first 3 (durably committed via ack-batch).
        let leases = r.lease_batch("q", "c", 3, now).unwrap();
        assert_eq!(leases.len(), 3);
        let acks: Vec<_> = leases
            .iter()
            .map(|l| (l.lease_id.clone(), Some(l.epoch)))
            .collect();
        let (acked, committed) = r.ack_batch("q", &acks).unwrap();
        assert_eq!(acked, 3);
        assert_eq!(committed.unwrap().committed_seq, 2);
    }

    // Reopen: committed offset recovered; lease resumes at seq 3, not 0.
    let r2 = Relay::new(disk_config(dir.path()));
    assert_eq!(
        r2.committed_offset("q").unwrap().unwrap().committed_seq,
        2,
        "committed offset is durable"
    );
    let next = r2.lease("q", "c", now).unwrap().unwrap();
    assert_eq!(
        next.seq, 3,
        "resumes after the committed watermark (no redelivery of 0..2)"
    );
}

// Phase 1 (delete-on-ack): an `Ack`-mode subject reclaims fully-acked segment
// files on the ack path, so disk usage tracks backlog depth, not throughput —
// while the un-acked tail stays durably retained and leasable.
#[test]
fn ack_retention_reclaims_acked_segments() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let r = Relay::new(segmented_config(dir.path(), 200));
    r.publish_batch("q", (0..20).map(|i| item(&format!("m{i}"))).collect(), now)
        .unwrap();
    let before = count_subject_segments(dir.path(), "q");
    assert!(before > 1, "rolled into multiple segments");

    // Lease + ack the contiguous prefix [0, 15): segments fully below the
    // watermark are reclaimed.
    let leases = r.lease_batch("q", "c", 15, now).unwrap();
    assert_eq!(leases.len(), 15);
    let acks: Vec<_> = leases
        .iter()
        .map(|l| (l.lease_id.clone(), Some(l.epoch)))
        .collect();
    let (acked, _) = r.ack_batch("q", &acks).unwrap();
    assert_eq!(acked, 15);

    let after = count_subject_segments(dir.path(), "q");
    assert!(
        after < before,
        "acked-prefix segment files reclaimed ({before} -> {after})"
    );
    assert_eq!(
        r.log_len("q").unwrap(),
        20,
        "len unchanged; only storage reclaimed"
    );

    // The first un-acked entry is still durably retained and leasable.
    let tail = r.lease("q", "c", now).unwrap().unwrap();
    assert_eq!(tail.seq, 15, "un-acked tail still available");
}

// Phase 1: ack-driven truncation never outruns durability — the committed
// watermark is persisted before segments are reclaimed, so a reopen recovers a
// consistent state with no lost or re-delivered acked entries.
#[test]
fn ack_truncation_recovers_consistently() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    {
        let r = Relay::new(segmented_config(dir.path(), 200));
        r.publish_batch("q", (0..20).map(|i| item(&format!("m{i}"))).collect(), now)
            .unwrap();
        let leases = r.lease_batch("q", "c", 15, now).unwrap();
        let acks: Vec<_> = leases
            .iter()
            .map(|l| (l.lease_id.clone(), Some(l.epoch)))
            .collect();
        let (acked, committed) = r.ack_batch("q", &acks).unwrap();
        assert_eq!(acked, 15);
        assert_eq!(committed.unwrap().committed_seq, 14);
    }

    // Reopen: committed watermark durable, resumes at seq 15 (no redelivery of
    // the truncated acked prefix, no loss of the un-acked tail).
    let r2 = Relay::new(segmented_config(dir.path(), 200));
    assert_eq!(
        r2.committed_offset("q").unwrap().unwrap().committed_seq,
        14,
        "committed offset durable across the truncation"
    );
    assert_eq!(r2.log_len("q").unwrap(), 20);
    let next = r2.lease("q", "c", now).unwrap().unwrap();
    assert_eq!(next.seq, 15, "resumes after the truncated, acked prefix");
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn spawn(dir: &std::path::Path, port: u16) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_relay"))
            .args([
                "--bind",
                &format!("127.0.0.1:{port}"),
                "--data-dir",
                dir.to_str().unwrap(),
                "--grace-secs",
                "1",
            ])
            .spawn()
            .expect("spawn relay process");
        Self(Some(child))
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

fn reserve_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

async fn wait_ready(port: u16) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .expect("build probe client");
    let url = format!("http://127.0.0.1:{port}/readyz");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("timed out waiting for relay process at port {port}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// End-to-end process lifecycle: starts the real relay binary on a temp data dir,
// publishes messages over h2c, consumes and acks some, restarts the process,
// and asserts unacked ones are redelivered while acked ones are not.
// The child process and temp dir are removed on both success and failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_relay_binary_recovers_unacked_after_restart() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let port1 = reserve_port();
    let guard = ChildGuard::spawn(dir.path(), port1);
    wait_ready(port1).await;

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .expect("build h2c client");

    // Publish N>0 messages over h2c.
    for id in ["m0", "m1", "m2"] {
        let resp = client
            .post(format!("http://127.0.0.1:{port1}/v1/q/publish"))
            .json(&json!({ "message_id": id, "payload": { "id": id } }))
            .send()
            .await
            .expect("send publish");
        assert!(resp.status().is_success(), "publish {id} succeeded");
    }

    // Consume and ack some (lease and ack m0).
    let lease_resp: LeaseResponse = client
        .post(format!("http://127.0.0.1:{port1}/v1/q/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .expect("send lease")
        .json()
        .await
        .expect("parse lease response");
    let lease = lease_resp.lease.expect("first lease granted");
    assert_eq!(lease.seq, 0, "first leased seq is 0");

    let ack_resp: AckResponse = client
        .post(format!("http://127.0.0.1:{port1}/v1/q/ack"))
        .json(&json!({ "lease_id": lease.lease_id, "epoch": lease.epoch }))
        .send()
        .await
        .expect("send ack")
        .json()
        .await
        .expect("parse ack response");
    assert!(ack_resp.acked, "lease seq 0 acked");

    // Restart the process against the same temp data dir.
    drop(guard);
    let port2 = reserve_port();
    let _guard2 = ChildGuard::spawn(dir.path(), port2);
    wait_ready(port2).await;

    // Assert the unacked messages are redelivered and the acked ones are not.
    let l1_resp: LeaseResponse = client
        .post(format!("http://127.0.0.1:{port2}/v1/q/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .expect("send lease 1 after restart")
        .json()
        .await
        .expect("parse lease 1 response");
    let l1 = l1_resp.lease.expect("lease for seq 1 granted");
    assert_eq!(
        l1.seq, 1,
        "resumes at unacked seq 1; acked seq 0 is not redelivered"
    );

    let l2_resp: LeaseResponse = client
        .post(format!("http://127.0.0.1:{port2}/v1/q/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .expect("send lease 2 after restart")
        .json()
        .await
        .expect("parse lease 2 response");
    let l2 = l2_resp.lease.expect("lease for seq 2 granted");
    assert_eq!(l2.seq, 2, "unacked seq 2 is delivered");

    let empty_resp: LeaseResponse = client
        .post(format!("http://127.0.0.1:{port2}/v1/q/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .expect("send empty lease")
        .json()
        .await
        .expect("parse empty lease response");
    assert!(empty_resp.lease.is_none(), "backlog is fully consumed");
}
// HANDWRITE-END
