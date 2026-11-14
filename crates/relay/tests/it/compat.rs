//! Compatibility pins (issue #173). They fail on a breaking change to two
//! things that clients and operators depend on: the operations of the served
//! HTTP API (the same document as `relay spec` and `/openapi.json`) and the
//! on-disk log format. Additive changes pass: a new path or a new field does
//! not fail these tests. Removing an operation, or changing the file name or
//! the frame payload fields of the log, does fail them.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use chrono::Utc;

use relay::shared_kernel::types::LogEntry;
use relay::{Log, RelayCoreConfig};

/// Every served operation, one `METHOD /path` line each (see
/// `tests/fixtures/openapi-operations.txt`).
const OPERATIONS: &str = include_str!("../fixtures/openapi-operations.txt");

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(args)
        .output()
        .expect("run relay binary")
}

fn stdout(args: &[&str]) -> String {
    let out = run(args);
    assert!(
        out.status.success(),
        "`relay {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

fn spec_doc() -> serde_json::Value {
    serde_json::from_str(&stdout(&["spec"])).expect("`relay spec` emits JSON")
}

/// The `METHOD /path` lines of an OpenAPI document. Only the HTTP methods
/// count; other keys under a path (`parameters`, `summary`) are not operations.
fn operation_lines(doc: &serde_json::Value) -> BTreeSet<String> {
    let mut lines = BTreeSet::new();
    if let Some(paths) = doc["paths"].as_object() {
        for (path, item) in paths {
            let Some(item) = item.as_object() else {
                continue;
            };
            for method in item.keys() {
                if matches!(method.as_str(), "get" | "put" | "post" | "delete" | "patch") {
                    lines.insert(format!("{} {}", method.to_ascii_uppercase(), path));
                }
            }
        }
    }
    lines
}

/// The non-empty lines of `snapshot` that `doc` does not serve.
fn missing_operations(snapshot: &str, doc: &serde_json::Value) -> Vec<String> {
    let served = operation_lines(doc);
    snapshot
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !served.contains(*line))
        .map(str::to_string)
        .collect()
}

#[test]
fn openapi_operations_are_not_removed() {
    let missing = missing_operations(OPERATIONS, &spec_doc());
    assert!(
        missing.is_empty(),
        "operations in the snapshot that the served API no longer has: {missing:?}"
    );
}

#[test]
fn missing_operations_reports_a_removed_operation() {
    let snapshot = "GET /v1/does-not-exist\nPOST /v1/{subject}/publish\n";
    assert_eq!(
        missing_operations(snapshot, &spec_doc()),
        vec!["GET /v1/does-not-exist".to_string()]
    );
}

fn cfg(dir: &std::path::Path, segment_bytes: u64) -> RelayCoreConfig {
    RelayCoreConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        segment_bytes,
        ram_ring_entries: 4,
        ..RelayCoreConfig::default()
    }
}

fn append_one(log: &mut Log, i: usize) {
    log.append(
        &format!("m{i}"),
        serde_json::json!({ "i": i }),
        BTreeMap::new(),
        Utc::now(),
    )
    .unwrap();
}

/// Writes one entry to a fresh data dir and returns the JSON of its frame
/// (the bytes after the 16-byte frame header of the one `.frames` segment).
fn first_frame_json() -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let mut log = Log::open(&cfg(dir.path(), 1 << 20), "s", 0).unwrap();
    append_one(&mut log, 0);
    drop(log);

    let segments: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("s__shard0__") && n.ends_with(".frames"))
        .collect();
    assert_eq!(segments.len(), 1, "one .frames segment: {segments:?}");

    let bytes = std::fs::read(dir.path().join(&segments[0])).unwrap();
    assert!(bytes.len() > 16, "a frame header and one frame");
    serde_json::Deserializer::from_slice(&bytes[16..])
        .into_iter::<serde_json::Value>()
        .next()
        .expect("one frame")
        .expect("frame payload is JSON")
}

#[test]
fn on_disk_log_format_is_stable() {
    let frame = first_frame_json();
    let mut keys: Vec<&str> = frame
        .as_object()
        .expect("frame payload is one JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "appended_at",
            "headers",
            "message_id",
            "not_before",
            "payload",
            "priority",
            "seq",
            "shard",
            "subject",
        ]
    );
}

#[test]
fn old_log_entry_without_optional_fields_still_loads() {
    let old = r#"{"seq":0,"message_id":"m0","subject":"s","shard":0,"payload":{"i":0},"appended_at":"2026-01-01T00:00:00Z"}"#;
    let entry: LogEntry = serde_json::from_str(old).expect("old entry loads");

    assert!(entry.headers.is_empty());
    assert_eq!(entry.not_before, None);
    // The default priority is the one an entry gets when the log appends it.
    let appended_priority = first_frame_json()["priority"].as_u64().expect("priority");
    assert_eq!(u64::from(entry.priority), appended_priority);
}
