// SPEC-MANAGED: tech-design/logic/core-durable-log-single-multi-broadcast-delivery-model.md#unit-test
// HANDWRITE-BEGIN gap="missing-generator:unit-test:ac3b11e2" tracker="pending-tracker" reason="Deterministic tests for the unit-test plan, including the #122 acceptance (both delivery models over the same log)."
//! Deterministic tests for the relay core, mapping 1:1 to the unit-test plan in
//! the TD: sequencing, idempotency, broadcast fan-out + replay-from-seq,
//! work-queue single-delivery, lease-expiry redelivery, ack/commit, and the
//! #122 acceptance — both delivery models over the same durable log.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Duration, Utc};

use relay::{Payload, Relay, RelayCoreConfig};

fn msg(name: &str) -> Payload {
    serde_json::json!({ "task": name })
}

fn ram() -> Relay {
    let mut c = RelayCoreConfig::in_memory();
    c.work_queue.redeliver_backoff_ms = 0; // pin immediate redelivery
    Relay::new(c)
}

// #166: a leased entry's stored body is retrievable by (subject, shard, seq) so
// a work-queue consumer knows what it leased.
#[test]
fn lease_exposes_entry_body() {
    let r = ram();
    let now = Utc::now();
    r.publish("q", "m0", msg("hello"), BTreeMap::new(), now)
        .unwrap();

    let l = r.lease("q", "c1", now).unwrap().expect("a lease");
    let entry = r
        .entry(&l.subject, l.shard, l.seq)
        .unwrap()
        .expect("leased entry body");

    assert_eq!(entry.message_id, "m0");
    assert_eq!(entry.payload, msg("hello"));
    assert_eq!(entry.seq, l.seq);
}

// case: sequencing — append assigns monotonic, gap-free seq.
#[test]
fn append_assigns_monotonic_seq() {
    let r = ram();
    let now = Utc::now();
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        let out = r.publish("s", id, msg("t"), BTreeMap::new(), now).unwrap();
        assert_eq!(out.seq, i as u64, "seq is the append index");
        assert!(!out.deduped);
    }
    assert_eq!(r.log_len("s").unwrap(), 3);
}

// case: idempotency — a repeated id appends nothing and returns the same seq.
#[test]
fn idempotent_dedupe() {
    let r = ram();
    let now = Utc::now();
    let first = r
        .publish("s", "dup", msg("t"), BTreeMap::new(), now)
        .unwrap();
    let second = r
        .publish("s", "dup", msg("t"), BTreeMap::new(), now)
        .unwrap();
    assert_eq!(
        first,
        relay::AppendOutcome {
            seq: 0,
            deduped: false
        }
    );
    assert_eq!(
        second,
        relay::AppendOutcome {
            seq: 0,
            deduped: true
        }
    );
    assert_eq!(r.log_len("s").unwrap(), 1);
}

// case: competing — each message is leased to exactly one consumer.
#[test]
fn workqueue_single_delivery() {
    let r = ram();
    let now = Utc::now();
    for id in ["m0", "m1", "m2"] {
        r.publish("q", id, msg("t"), BTreeMap::new(), now).unwrap();
    }
    let mut leased: Vec<(String, u64)> = Vec::new();
    let mut toggle = true;
    loop {
        let consumer = if toggle { "c1" } else { "c2" };
        match r.lease("q", consumer, now).unwrap() {
            Some(l) => leased.push((consumer.to_string(), l.seq)),
            None => break,
        }
        toggle = !toggle;
    }
    let seqs: BTreeSet<u64> = leased.iter().map(|(_, s)| *s).collect();
    assert_eq!(seqs, BTreeSet::from([0, 1, 2]), "all messages leased");
    assert_eq!(
        leased.len(),
        seqs.len(),
        "no message leased to two consumers"
    );
}

// case: redelivery — an expired lease becomes redelivery-eligible, attempt++.
#[test]
fn lease_expiry_redelivers() {
    let r = ram();
    let now = Utc::now();
    r.publish("q", "m0", msg("t"), BTreeMap::new(), now)
        .unwrap();

    let first = r.lease("q", "c1", now).unwrap().expect("a lease");
    assert_eq!(first.attempt, 1);
    // nothing else available while it is leased
    assert!(r.lease("q", "c2", now).unwrap().is_none());

    // past the lease TTL, reclaim makes it eligible again
    let later = now
        + Duration::milliseconds(RelayCoreConfig::default().work_queue.lease_ttl_ms as i64)
        + Duration::seconds(1);
    assert_eq!(r.reclaim_expired("q", later).unwrap(), vec![0]);

    let redelivered = r.lease("q", "c2", later).unwrap().expect("redelivered");
    assert_eq!(redelivered.seq, 0);
    assert_eq!(redelivered.attempt, 2, "attempt carried forward");
    assert_eq!(redelivered.consumer_id, "c2");
}

// case: ack/commit — acking advances the committed offset and stops redelivery.
#[test]
fn ack_advances_committed_offset() {
    let r = ram();
    let now = Utc::now();
    r.publish("q", "m0", msg("t"), BTreeMap::new(), now)
        .unwrap();
    r.publish("q", "m1", msg("t"), BTreeMap::new(), now)
        .unwrap();

    assert!(r.committed_offset("q").unwrap().is_none());

    let l0 = r.lease("q", "c1", now).unwrap().unwrap();
    assert!(r.ack("q", &l0.lease_id, None).unwrap());
    assert_eq!(r.committed_offset("q").unwrap().unwrap().committed_seq, 0);

    let l1 = r.lease("q", "c1", now).unwrap().unwrap();
    assert_eq!(l1.seq, 1, "acked entry is not re-offered");
    assert!(r.ack("q", &l1.lease_id, None).unwrap());
    assert_eq!(r.committed_offset("q").unwrap().unwrap().committed_seq, 1);
}

// single-cast work-queue: competing consumers each lease a message exactly once
// over the durable log, advancing the committed offset.
#[test]
fn single_cast_work_queue_drains_once() {
    let r = ram();
    let now = Utc::now();
    for id in ["e0", "e1", "e2"] {
        r.publish("events", id, msg("t"), BTreeMap::new(), now)
            .unwrap();
    }

    let mut acked: BTreeSet<u64> = BTreeSet::new();
    while let Some(l) = r.lease("events", "worker", now).unwrap() {
        assert!(r.ack("events", &l.lease_id, None).unwrap());
        acked.insert(l.seq);
    }
    assert_eq!(acked, BTreeSet::from([0, 1, 2]));
    assert_eq!(
        r.committed_offset("events").unwrap().unwrap().committed_seq,
        2
    );
}

// durability — entries and the dedupe index survive reopening from disk.
#[test]
fn durable_log_recovers_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = RelayCoreConfig::in_memory();
    cfg.data_dir = dir.path().to_string_lossy().into_owned();
    let now = Utc::now();

    {
        let r = Relay::new(cfg.clone());
        r.publish("s", "a", msg("t"), BTreeMap::new(), now).unwrap();
        r.publish("s", "b", msg("t"), BTreeMap::new(), now).unwrap();
    }

    let r2 = Relay::new(cfg);
    assert_eq!(r2.log_len("s").unwrap(), 2, "entries recovered from disk");
    let out = r2
        .publish("s", "a", msg("t"), BTreeMap::new(), now)
        .unwrap();
    assert!(out.deduped, "dedupe index recovered");
    assert_eq!(out.seq, 0);
}

// claim: per-subject-shard-append-ordering
// Expected Result: observes per subject shard append ordering
#[test]
fn per_subject_shard_append_ordering() {
    let mut cfg = RelayCoreConfig::in_memory();
    cfg.default_shards = 4;
    let r = Relay::new(cfg);
    let now = Utc::now();

    for i in 0..200 {
        r.publish("s", &format!("m{i}"), msg("t"), BTreeMap::new(), now)
            .unwrap();
    }
    assert_eq!(r.log_len("s").unwrap(), 200);

    let mut leased_by_shard: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    while let Some(l) = r.lease("s", "c", now).unwrap() {
        leased_by_shard.entry(l.shard).or_default().push(l.seq);
        assert!(r.ack("s", &l.lease_id, Some(l.epoch)).unwrap());
    }

    assert_eq!(
        leased_by_shard.values().map(|v| v.len()).sum::<usize>(),
        200,
        "all 200 messages drained"
    );

    for shard in 0..4 {
        let seqs = leased_by_shard.get(&shard).cloned().unwrap_or_default();
        let k = seqs.len() as u64;
        assert!(k > 0, "shard {shard} received messages");
        let expected_seqs: Vec<u64> = (0..k).collect();
        assert_eq!(
            seqs, expected_seqs,
            "leased seq values are exactly 0..k with no gap"
        );

        let mut published_indices: Vec<usize> = Vec::new();
        for seq in 0..k {
            let entry = r
                .entry("s", shard, seq)
                .unwrap()
                .expect("entry must exist for seq");
            assert_eq!(entry.seq, seq);
            let idx: usize = entry
                .message_id
                .strip_prefix('m')
                .expect("m prefix")
                .parse()
                .expect("integer suffix");
            published_indices.push(idx);
        }
        let mut sorted_indices = published_indices.clone();
        sorted_indices.sort_unstable();
        assert_eq!(
            published_indices, sorted_indices,
            "entry(subject, shard, seq).message_id lists messages in published order"
        );
    }
}

// case: interleaved publishes to two subjects keep independent seq spaces starting at 0.
#[test]
fn interleaved_subjects_independent_seq_spaces() {
    let r = ram();
    let now = Utc::now();
    for i in 0..50 {
        let out1 = r
            .publish("s1", &format!("a{i}"), msg("t"), BTreeMap::new(), now)
            .unwrap();
        assert_eq!(
            out1.seq, i as u64,
            "s1 seq starts at 0 and increments with each publish"
        );
        assert!(!out1.deduped);

        let out2 = r
            .publish("s2", &format!("b{i}"), msg("t"), BTreeMap::new(), now)
            .unwrap();
        assert_eq!(
            out2.seq, i as u64,
            "s2 seq starts at 0 and increments with each publish"
        );
        assert!(!out2.deduped);
    }
    assert_eq!(
        r.log_len("s1").unwrap(),
        50,
        "log_len per subject equals count published to s1"
    );
    assert_eq!(
        r.log_len("s2").unwrap(),
        50,
        "log_len per subject equals count published to s2"
    );
}

// case: deduped republish preserves original seq and next publish continues gap-free.
#[test]
fn deduped_republish_preserves_shard_seq_order() {
    let r = ram();
    let now = Utc::now();
    for i in 0..10 {
        let out = r
            .publish("s", &format!("m{i}"), msg("t"), BTreeMap::new(), now)
            .unwrap();
        assert_eq!(out.seq, i as u64);
        assert!(!out.deduped);
    }

    let re3 = r
        .publish("s", "m3", msg("t"), BTreeMap::new(), now)
        .unwrap();
    assert!(re3.deduped, "m3 republish returns deduped: true");
    assert_eq!(re3.seq, 3, "m3 republish returns original seq");

    let re7 = r
        .publish("s", "m7", msg("t"), BTreeMap::new(), now)
        .unwrap();
    assert!(re7.deduped, "m7 republish returns deduped: true");
    assert_eq!(re7.seq, 7, "m7 republish returns original seq");

    let out10 = r
        .publish("s", "m10", msg("t"), BTreeMap::new(), now)
        .unwrap();
    assert!(!out10.deduped);
    assert_eq!(
        out10.seq, 10,
        "m10 gets the next seq of its shard, preserving order and gap-freeness"
    );
    assert_eq!(r.log_len("s").unwrap(), 11);
}

// case: durable reopen preserves per-shard entry order and subsequent publish continues shard seq.
#[test]
fn durable_reopen_preserves_per_shard_order_and_seq() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_string_lossy().into_owned();
    let cfg = RelayCoreConfig {
        data_dir,
        default_shards: 4,
        ..RelayCoreConfig::default()
    };
    let now = Utc::now();

    let mut expected_entries: BTreeMap<u32, Vec<(u64, String)>> = BTreeMap::new();
    {
        let r = Relay::new(cfg.clone());
        for i in 0..50 {
            r.publish("s", &format!("m{i}"), msg("t"), BTreeMap::new(), now)
                .unwrap();
        }
        assert_eq!(r.log_len("s").unwrap(), 50);

        for shard in 0..4 {
            let mut seq = 0;
            while let Ok(Some(entry)) = r.entry("s", shard, seq) {
                expected_entries
                    .entry(shard)
                    .or_default()
                    .push((entry.seq, entry.message_id));
                seq += 1;
            }
        }
    }

    let r2 = Relay::new(cfg);
    assert_eq!(
        r2.log_len("s").unwrap(),
        50,
        "log_len unchanged after reopen"
    );

    for shard in 0..4 {
        let expected = expected_entries.get(&shard).cloned().unwrap_or_default();
        for (expected_seq, expected_id) in &expected {
            let entry = r2
                .entry("s", shard, *expected_seq)
                .unwrap()
                .expect("entry exists on reopened relay");
            assert_eq!(entry.seq, *expected_seq);
            assert_eq!(&entry.message_id, expected_id);
        }
    }

    let new_out = r2
        .publish("s", "m50", msg("t"), BTreeMap::new(), now)
        .unwrap();
    assert!(!new_out.deduped);

    let mut target_shard = None;
    for shard in 0..4 {
        if let Ok(Some(entry)) = r2.entry("s", shard, new_out.seq) {
            if entry.message_id == "m50" {
                target_shard = Some(shard);
                break;
            }
        }
    }
    let shard = target_shard.expect("m50 found in one of the shards");
    let prior_count = expected_entries.get(&shard).map_or(0, |v| v.len() as u64);
    assert_eq!(
        new_out.seq, prior_count,
        "new publish continues the shard's seq"
    );
    assert_eq!(r2.log_len("s").unwrap(), 51);
}
// HANDWRITE-END
