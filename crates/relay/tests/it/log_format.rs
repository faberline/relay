//! The relay log stores each segment as a core `storage-durable` framed-log
//! file (`.frames`), with a torn tail repaired on reopen, and refuses a legacy
//! NDJSON data dir without reading or deleting it (#129).

use std::collections::BTreeMap;
use std::io::Write;

use chrono::Utc;
use proptest::prelude::*;

use relay::{Log, LogEntry, RelayCoreConfig};

fn cfg(dir: &std::path::Path, segment_bytes: u64) -> RelayCoreConfig {
    RelayCoreConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        segment_bytes,
        ram_ring_entries: 4,
        ..RelayCoreConfig::default()
    }
}

fn append(log: &mut Log, i: usize) {
    log.append(
        &format!("m{i}"),
        serde_json::json!({ "i": i }),
        BTreeMap::new(),
        Utc::now(),
    )
    .unwrap();
}

#[test]
fn segments_are_core_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = Log::open(&cfg(dir.path(), 200), "s", 0).unwrap();
    for i in 0..20 {
        append(&mut log, i);
    }
    drop(log);

    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let frames = names.iter().filter(|n| n.ends_with(".frames")).count();
    assert!(
        frames >= 2,
        "rolled into at least two .frames files: {names:?}"
    );
    assert!(
        names.iter().all(|n| !n.ends_with(".ndjson")),
        "no NDJSON segment is written: {names:?}"
    );

    // Core's reader decodes the first segment. `read_frames(path, 0)` skips
    // seq <= 0, so seq 0 is not returned and the first frame is seq 1.
    let first = dir.path().join("s__shard0__0.frames");
    let read = storage_durable::FramedLogReader::read_frames(&first, 0).unwrap();
    assert!(!read.is_empty(), "first segment holds frames");
    assert_eq!(read[0].seq, 1);
    let payload: serde_json::Value = serde_json::from_slice(&read[0].payload).unwrap();
    assert!(
        payload.is_object() && payload.get("message_id").is_some(),
        "frame payload is one JSON entry: {payload}"
    );
}

#[test]
fn torn_tail_is_truncated_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = Log::open(&cfg(dir.path(), 1_000_000), "s", 0).unwrap();
    for i in 0..5 {
        append(&mut log, i);
    }
    drop(log);

    // A partial frame header at the end of the single segment.
    let path = dir.path().join("s__shard0__0.frames");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[0xAB, 0xCD, 0xEF])
        .unwrap();

    let mut log = Log::open(&cfg(dir.path(), 1_000_000), "s", 0).unwrap();
    assert_eq!(log.len(), 5);
    let seqs: Vec<u64> = log.range(0).unwrap().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (0..5).collect::<Vec<u64>>());
    let next = log
        .append(
            "m5",
            serde_json::json!({ "i": 5 }),
            BTreeMap::new(),
            Utc::now(),
        )
        .unwrap();
    assert_eq!(next.seq, 5);
}

#[test]
fn legacy_ndjson_dir_fails_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("s__shard0__0.ndjson");
    std::fs::write(&legacy, "{}\n").unwrap();

    let err = match Log::open(&cfg(dir.path(), 200), "s", 0) {
        Ok(_) => panic!("a legacy NDJSON data dir must not open"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        err.to_string().contains("legacy NDJSON"),
        "clear error names the legacy format: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(&legacy).unwrap(),
        "{}\n",
        "the legacy file is neither read into the log nor deleted"
    );
}

#[test]
fn cold_read_after_reopen_uses_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = Log::open(&cfg(dir.path(), 200), "s", 0).unwrap();
    for i in 0..30 {
        append(&mut log, i);
    }
    drop(log);

    let log = Log::open(&cfg(dir.path(), 200), "s", 0).unwrap();
    let first = log.entry(0).unwrap().expect("seq 0 is on disk");
    assert_eq!(first.seq, 0);
    assert_eq!(log.range(0).unwrap().len(), 30);
}

fn fixed_now() -> chrono::DateTime<Utc> {
    chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn log_entries_round_trip_after_reopen(
        items in proptest::collection::btree_map(".{1,24}", (json_value(), header_map()), 1..12)
    ) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg(dir.path(), 300);
        let now = fixed_now();
        let mut expected = Vec::new();
        {
            let mut log = Log::open(&cfg, "s", 0).unwrap();
            for (index, (id, (payload, headers))) in items.iter().enumerate() {
                let out = log.append(id, payload.clone(), headers.clone(), now).unwrap();
                prop_assert_eq!(out.seq, index as u64);
                prop_assert!(!out.deduped);
                expected.push(LogEntry {
                    seq: index as u64,
                    message_id: id.clone(),
                    subject: "s".into(),
                    shard: 0,
                    payload: payload.clone(),
                    headers: headers.clone(),
                    appended_at: now,
                    not_before: None,
                    priority: relay::DEFAULT_PRIORITY,
                });
            }
        }
        let log = Log::open(&cfg, "s", 0).unwrap();
        let read = log.range(0).unwrap();
        prop_assert!(
            entries_equal(&read, &expected).is_ok(),
            "{:?}",
            entries_equal(&read, &expected)
        );
        prop_assert_eq!(log.len(), expected.len() as u64);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn log_entry_survives_json_round_trip(
        payload in json_value(),
        headers in header_map(),
        id in ".{1,24}",
        seq in any::<u64>(),
        priority in any::<u8>(),
    ) {
        let entry = LogEntry {
            seq,
            message_id: id,
            subject: "s".into(),
            shard: 0,
            payload,
            headers,
            appended_at: fixed_now(),
            not_before: None,
            priority,
        };
        let text = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&text).unwrap();
        prop_assert_eq!(back, entry);
    }
}

#[test]
fn entries_equal_rejects_different_entries() {
    let a = LogEntry {
        seq: 0,
        message_id: "m0".into(),
        subject: "s".into(),
        shard: 0,
        payload: serde_json::json!({ "a": 1 }),
        headers: BTreeMap::new(),
        appended_at: fixed_now(),
        not_before: None,
        priority: relay::DEFAULT_PRIORITY,
    };
    let mut b = a.clone();
    b.payload = serde_json::json!({ "a": 2 });
    assert!(entries_equal(std::slice::from_ref(&a), std::slice::from_ref(&a)).is_ok());
    assert!(entries_equal(std::slice::from_ref(&a), std::slice::from_ref(&b)).is_err());
    assert!(entries_equal(std::slice::from_ref(&a), &[]).is_err());
}

fn json_value() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::from),
        any::<i64>().prop_map(serde_json::Value::from),
        ".{0,16}".prop_map(serde_json::Value::from),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(serde_json::Value::Array),
            proptest::collection::btree_map("[a-z]{1,6}", inner, 0..4)
                .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
        ]
    })
}

fn header_map() -> impl Strategy<Value = BTreeMap<String, String>> {
    proptest::collection::btree_map("[a-z-]{1,8}", ".{0,12}", 0..4)
}

fn entries_equal(left: &[LogEntry], right: &[LogEntry]) -> Result<(), String> {
    if left.len() != right.len() {
        return Err(format!("length differs: {} vs {}", left.len(), right.len()));
    }
    match left.iter().zip(right).position(|(l, r)| l != r) {
        Some(i) => Err(format!(
            "entry {i} differs: {:?} vs {:?}",
            left[i], right[i]
        )),
        None => Ok(()),
    }
}
