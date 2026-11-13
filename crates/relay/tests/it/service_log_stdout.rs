//! The real `relay` binary with JSON logs (`--log-format json`). Every stdout
//! line must be an `axiom.service.log.v1` record, and a request that sends a
//! W3C `traceparent` must keep its trace id on its log line.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const REGISTRY: &str = r#"{
    "writer-token": {"subject": "producer", "roles": {"jobs": "write"}},
    "reader-token": {"subject": "worker", "roles": {"jobs": "read"}}
}"#;

const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const SEVERITIES: [&str; 5] = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
const REQUIRED: [&str; 7] = [
    "schema",
    "timestamp",
    "severity",
    "service",
    "event",
    "message",
    "attributes",
];

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Checks one stdout line against `axiom.service.log.v1` by hand, without a
/// JSON Schema crate. Returns the parsed object on success.
fn check_line(line: &str) -> Result<Value, String> {
    let value: Value =
        serde_json::from_str(line).map_err(|e| format!("line is not JSON ({e}): {line}"))?;
    if !value.is_object() {
        return Err(format!("line is not a JSON object: {line}"));
    }
    for field in REQUIRED {
        if value.get(field).is_none() {
            return Err(format!("missing required field `{field}`: {line}"));
        }
    }
    if value["schema"] != "axiom.service.log.v1" {
        return Err(format!("schema is not axiom.service.log.v1: {line}"));
    }
    match value["severity"].as_str() {
        Some(s) if SEVERITIES.contains(&s) => {}
        _ => return Err(format!("severity is not one of TRACE..ERROR: {line}")),
    }
    if !value["service"].is_object() || !value["service"].to_string().contains("relay") {
        return Err(format!("service is not an object naming relay: {line}"));
    }
    for field in ["timestamp", "event", "message"] {
        if !value[field].is_string() {
            return Err(format!("`{field}` is not a string: {line}"));
        }
    }
    if !value["attributes"].is_object() {
        return Err(format!("attributes is not an object: {line}"));
    }
    if let Some(trace) = value.get("trace_id") {
        if !trace.as_str().is_some_and(|t| is_hex(t, 32)) {
            return Err(format!("trace_id is not 32 hex characters: {line}"));
        }
    }
    if let Some(span) = value.get("span_id") {
        if !span.as_str().is_some_and(|s| is_hex(s, 16)) {
            return Err(format!("span_id is not 16 hex characters: {line}"));
        }
    }
    Ok(value)
}

/// Owns the relay child. Dropping it kills and reaps the process, so the child
/// is removed on every exit path, including a failed assertion.
struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn get(&mut self) -> &mut Child {
        self.0.as_mut().expect("relay child is owned by the guard")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Reads a pipe to the end on its own thread, so a full pipe never blocks relay.
fn read_lines<R: Read + Send + 'static>(stream: R) -> JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        BufReader::new(stream)
            .lines()
            .map_while(Result::ok)
            .collect()
    })
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
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for relay at port {port}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn publish(
    client: &reqwest::Client,
    port: u16,
    token: Option<&str>,
    traceparent: Option<&str>,
    id: &str,
) -> reqwest::StatusCode {
    let mut req = client
        .post(format!("http://127.0.0.1:{port}/v1/jobs/publish"))
        .json(&json!({ "message_id": id, "payload": { "n": id } }));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(tp) = traceparent {
        req = req.header("traceparent", tp);
    }
    req.send().await.expect("publish request").status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_relay_json_logs_match_service_log_v1() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = dir.path().join("token-registry.json");
    std::fs::write(&registry, REGISTRY).expect("write token registry");
    let port = reserve_port();

    // `dir` is declared before `guard`, so the child is killed before the dir is removed.
    let mut guard = ChildGuard(Some(
        Command::new(env!("CARGO_BIN_EXE_relay"))
            .args([
                "--bind",
                &format!("127.0.0.1:{port}"),
                "--data-dir",
                dir.path().to_str().expect("utf-8 data dir"),
                "--grace-secs",
                "1",
                "--log-format",
                "json",
                "--auth",
                "required",
                "--token-registry-file",
                registry.to_str().expect("utf-8 registry path"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn relay process"),
    ));
    let stdout_reader = read_lines(guard.get().stdout.take().expect("stdout pipe"));
    let stderr_reader = read_lines(guard.get().stderr.take().expect("stderr pipe"));

    wait_ready(port).await;
    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("build client");

    assert!(
        publish(&client, port, Some("writer-token"), None, "m-ok")
            .await
            .is_success(),
        "authorized publish must succeed"
    );
    assert_eq!(
        publish(&client, port, None, None, "m-anon").await,
        reqwest::StatusCode::UNAUTHORIZED,
        "missing token must be 401"
    );
    assert_eq!(
        publish(&client, port, Some("reader-token"), None, "m-read").await,
        reqwest::StatusCode::FORBIDDEN,
        "reader token publish must be 403"
    );
    assert!(
        publish(
            &client,
            port,
            Some("writer-token"),
            Some(TRACEPARENT),
            "m-traced"
        )
        .await
        .is_success(),
        "traced publish must succeed"
    );

    let pid = guard.get().id();
    let killed = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("run kill");
    assert!(killed.success(), "kill -TERM {pid} failed");
    let deadline = Instant::now() + Duration::from_secs(11);
    while guard.get().try_wait().expect("try_wait").is_none() {
        assert!(
            Instant::now() < deadline,
            "relay did not exit within the grace window"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let stdout = stdout_reader.join().expect("stdout reader");
    let stderr = stderr_reader.join().expect("stderr reader");

    let parsed: Vec<Value> = stdout
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            check_line(line).unwrap_or_else(|e| panic!("stdout line failed the contract: {e}"))
        })
        .collect();
    assert!(!parsed.is_empty(), "relay wrote no stdout log lines");
    eprintln!("parsed {} stdout log lines", parsed.len());

    let message = |v: &Value| v["message"].as_str().unwrap_or("").to_string();
    let observed = parsed
        .iter()
        .map(|v| format!("{} / {}", v["event"].as_str().unwrap_or(""), message(v)))
        .collect::<Vec<_>>()
        .join(" | ");
    eprintln!("observed events: {observed}");

    assert!(
        parsed
            .iter()
            .any(|v| message(v).contains("relay listening")),
        "missing lifecycle line `relay listening`; observed: {observed}"
    );
    assert!(
        parsed
            .iter()
            .any(|v| message(v).contains("request auth resolved")),
        "missing line `request auth resolved`; observed: {observed}"
    );
    assert!(
        parsed
            .iter()
            .any(|v| v["attributes"]["target"] == "http.access"
                && matches!(v["attributes"]["status"].as_u64(), Some(401 | 403))),
        "missing 401/403 access line with target http.access; observed: {observed}"
    );
    assert!(
        parsed
            .iter()
            .any(|v| v["attributes"]["target"] == "http.access"
                && matches!(v["attributes"]["status"].as_u64(), Some(200..=299))),
        "missing 2xx access line with target http.access; observed: {observed}"
    );
    assert!(
        parsed.iter().any(|v| v["trace_id"] == TRACE_ID),
        "no line carries trace_id {TRACE_ID}; observed: {observed}"
    );

    let registry_path = registry.to_string_lossy().into_owned();
    for line in stdout.iter().chain(stderr.iter()) {
        for secret in ["writer-token", "reader-token", registry_path.as_str()] {
            assert!(
                !line.contains(secret),
                "a log line leaks `{secret}`: {line}"
            );
        }
    }
}

#[test]
fn check_line_rejects_non_contract_lines() {
    assert!(check_line(r#"{"schema":"x"}"#).is_err());
    assert!(check_line("2026-11-10T00:00:00Z INFO relay listening").is_err());
    let valid = r#"{"schema":"axiom.service.log.v1","timestamp":"2026-11-10T00:00:00Z","severity":"INFO","service":{"name":"relay"},"event":"e","message":"m","attributes":{}}"#;
    assert!(check_line(valid).is_ok(), "control line must pass");
    let loud = valid.replace("\"INFO\"", "\"LOUD\"");
    assert!(check_line(&loud).is_err(), "unknown severity must fail");
}
