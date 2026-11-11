//! Log on-disk format (#129): segments are core `storage-durable` framed files
//! (`.frames`), a torn tail is repaired on reopen, and a legacy NDJSON data dir
//! fails to open with a clear error instead of being read or deleted.

use std::collections::BTreeMap;
use std::io::Write;

use chrono::Utc;

use relay::{Log, RelayCoreConfig};

fn seg_cfg(dir: &std::path::Path, segment_bytes: u64) -> RelayCoreConfig {
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

fn files_with_suffix(dir: &std::path::Path, suffix: &str) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(suffix))
        .collect();
    out.sort();
    out
}

// Segments are written as core framed logs, not NDJSON.
#[test]
fn segments_are_core_frames() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut log = Log::open(&seg_cfg(dir.path(), 200), "s", 0).unwrap();
        for i in 0..20 {
            append(&mut log, i);
        }
    }
    let frames_files = files_with_suffix(dir.path(), ".frames");
    assert!(
        frames_files.len() >= 2,
        "expected at least 2 .frames segments, found {}",
        frames_files.len()
    );
    assert!(
        files_with_suffix(dir.path(), ".ndjson").is_empty(),
        "no .ndjson segment may be written"
    );
    let frames = storage_durable::FramedLogReader::read_frames(&frames_files[0], 0).unwrap();
    assert!(!frames.is_empty(), "first segment must hold frames");
    assert_eq!(frames[0].seq, 1, "read_frames(0) skips seq 0");
    let payload: serde_json::Value = serde_json::from_slice(&frames[0].payload).unwrap();
    assert!(
        payload.is_object() && payload.get("message_id").is_some(),
        "frame payload must be the LogEntry JSON with message_id"
    );
}

// A partial frame at the end of the active segment is cut off on reopen.
#[test]
fn torn_tail_is_truncated_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = seg_cfg(dir.path(), 1_000_000);
    {
        let mut log = Log::open(&cfg, "s", 0).unwrap();
        for i in 0..5 {
            append(&mut log, i);
        }
    }
    let frames_files = files_with_suffix(dir.path(), ".frames");
    assert_eq!(frames_files.len(), 1, "single-segment log");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&frames_files[0])
        .unwrap();
    f.write_all(&[0xAB, 0xCD, 0xEF]).unwrap();
    drop(f);

    let mut log = Log::open(&cfg, "s", 0).unwrap();
    assert_eq!(log.len(), 5, "torn tail must not count as an entry");
    assert_eq!(
        log.range(0)
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        (0..5).collect::<Vec<u64>>()
    );
    append(&mut log, 5);
    assert_eq!(log.len(), 6);
    assert_eq!(log.entry(5).unwrap().map(|e| e.seq), Some(5));
}

// An old NDJSON data dir is refused, not read and not deleted.
#[test]
fn legacy_ndjson_dir_fails_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("s__shard0__0.ndjson");
    std::fs::write(&legacy, "{}\n").unwrap();

    let cfg = seg_cfg(dir.path(), 200);
    match Log::open(&cfg, "s", 0) {
        Ok(_) => panic!("a legacy NDJSON data dir must not open"),
        Err(err) => {
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
            assert!(
                err.to_string().contains("legacy NDJSON"),
                "error must name the legacy format: {err}"
            );
        }
    }
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "{}\n");
}

// Cold reads after a reopen come from the framed segments.
#[test]
fn cold_read_after_reopen_uses_frames() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = seg_cfg(dir.path(), 200);
    {
        let mut log = Log::open(&cfg, "s", 0).unwrap();
        for i in 0..30 {
            append(&mut log, i);
        }
    }
    let log = Log::open(&cfg, "s", 0).unwrap();
    let first = log.entry(0).unwrap();
    assert_eq!(first.map(|e| e.seq), Some(0));
    assert_eq!(log.range(0).unwrap().len(), 30);
}
