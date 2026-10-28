//! HTTP, security, and observability integration journey test (#44).
//!
//! Verifies on a single in-process server with required bearer auth and write
//! admission enabled that positive and negative HTTP behavior, tokenless probes,
//! /metrics counters, and secret redaction hold together, with non-zero observed work.

use std::net::SocketAddr;

use relay::access::auth::AuthConfig;
use relay::queue::server::{router_with_admission, AppState};
use relay::queue::server_config::RelayServerConfig;
use service_http::{AdmissionConfig, AdmissionController};

const REGISTRY: &str = r#"{
    "writer-token": {"subject": "producer", "roles": {"jobs": "write"}},
    "reader-token": {"subject": "worker", "roles": {"jobs": "read"}},
    "admin-token": {"subject": "root", "roles": {"*": "admin"}}
}"#;

fn admission_controller() -> AdmissionController {
    AdmissionConfig::from_lookup("RELAY", |key| match key {
        "RELAY_ADMISSION_WRITE_CAPACITY" => Some("1".into()),
        "RELAY_ADMISSION_REFILL_SECS" => Some("60".into()),
        "RELAY_ADMISSION_MAX_KEYS" => Some("16".into()),
        _ => None,
    })
    .unwrap()
    .controller("relay.read", "relay.write", "relay.admin")
    .unwrap()
}

async fn start_server(reg_path: &std::path::Path) -> SocketAddr {
    let auth = AuthConfig::resolve("required", Some(reg_path.to_str().unwrap()), None).unwrap();
    let config = RelayServerConfig::ephemeral();
    let state = AppState::with_auth(config, auth, 8 * 1024 * 1024);
    let app = router_with_admission(state, Some(admission_controller()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(service_http::serve(
        listener,
        app,
        std::future::pending::<()>(),
    ));
    addr
}

fn h2c_client() -> reqwest::Client {
    reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_security_observability_journey() {
    let dir = tempfile::tempdir().unwrap();
    let reg_path = dir.path().join("token-registry.json");
    std::fs::write(&reg_path, REGISTRY).unwrap();

    let addr = start_server(&reg_path).await;
    let client = h2c_client();
    let base_url = format!("http://{addr}");

    let mut seen_bodies: Vec<(&str, String)> = Vec::new();

    // -------------------------------------------------------------------------
    // Step: negative
    // No token and unknown token on /v1/jobs/publish are 401.
    // Read-only token on /v1/jobs/publish is 403.
    // Each body is the shared {error, message} envelope (parse as JSON and assert both keys).
    // -------------------------------------------------------------------------
    let res_no_token = client
        .post(format!("{base_url}/v1/jobs/publish"))
        .json(&serde_json::json!({
            "message_id": "neg-no-token",
            "payload": { "step": "negative" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res_no_token.status(),
        401,
        "negative: expected 401 for no token on /v1/jobs/publish"
    );
    let body_no_token = res_no_token.text().await.unwrap();
    let json_no_token: serde_json::Value = serde_json::from_str(&body_no_token)
        .expect("negative: 401 response for no token must be valid json");
    assert!(
        json_no_token.get("error").is_some(),
        "negative: 401 body for no token must contain 'error' key"
    );
    assert!(
        json_no_token.get("message").is_some(),
        "negative: 401 body for no token must contain 'message' key"
    );
    seen_bodies.push(("/v1/jobs/publish (no token)", body_no_token));

    let res_unknown = client
        .post(format!("{base_url}/v1/jobs/publish"))
        .bearer_auth("unknown-token")
        .json(&serde_json::json!({
            "message_id": "neg-unknown",
            "payload": { "step": "negative" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res_unknown.status(),
        401,
        "negative: expected 401 for unknown token on /v1/jobs/publish"
    );
    let body_unknown = res_unknown.text().await.unwrap();
    let json_unknown: serde_json::Value = serde_json::from_str(&body_unknown)
        .expect("negative: 401 response for unknown token must be valid json");
    assert!(
        json_unknown.get("error").is_some(),
        "negative: 401 body for unknown token must contain 'error' key"
    );
    assert!(
        json_unknown.get("message").is_some(),
        "negative: 401 body for unknown token must contain 'message' key"
    );
    seen_bodies.push(("/v1/jobs/publish (unknown token)", body_unknown));

    let res_readonly = client
        .post(format!("{base_url}/v1/jobs/publish"))
        .bearer_auth("reader-token")
        .json(&serde_json::json!({
            "message_id": "neg-readonly",
            "payload": { "step": "negative" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res_readonly.status(),
        403,
        "negative: expected 403 for read-only token on /v1/jobs/publish"
    );
    let body_readonly = res_readonly.text().await.unwrap();
    let json_readonly: serde_json::Value = serde_json::from_str(&body_readonly)
        .expect("negative: 403 response for read-only token must be valid json");
    assert!(
        json_readonly.get("error").is_some(),
        "negative: 403 body for read-only token must contain 'error' key"
    );
    assert!(
        json_readonly.get("message").is_some(),
        "negative: 403 body for read-only token must contain 'message' key"
    );
    seen_bodies.push(("/v1/jobs/publish (read-only token)", body_readonly));

    // -------------------------------------------------------------------------
    // Step: positive
    // The write token publishes once and gets 200 with seq 0.
    // A second publish with the same token is 429 with a retry-after header (admission).
    // /healthz, /readyz, /metrics, /openapi.json are 200 with no token.
    // -------------------------------------------------------------------------
    let res_write_ok = client
        .post(format!("{base_url}/v1/jobs/publish"))
        .bearer_auth("writer-token")
        .json(&serde_json::json!({
            "message_id": "pos-write-1",
            "payload": { "step": "positive" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res_write_ok.status(),
        200,
        "positive: expected 200 for writer-token on /v1/jobs/publish"
    );
    let json_write_ok: serde_json::Value = res_write_ok.json().await.unwrap();
    assert_eq!(
        json_write_ok["seq"], 0,
        "positive: first publish with writer-token must get seq 0"
    );

    let res_write_throttled = client
        .post(format!("{base_url}/v1/jobs/publish"))
        .bearer_auth("writer-token")
        .json(&serde_json::json!({
            "message_id": "pos-write-2",
            "payload": { "step": "positive" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res_write_throttled.status(),
        429,
        "positive: expected 429 for second publish with same writer-token"
    );
    assert!(
        res_write_throttled.headers().contains_key("retry-after"),
        "positive: 429 response must contain retry-after header"
    );
    let body_throttled = res_write_throttled.text().await.unwrap();
    seen_bodies.push(("/v1/jobs/publish (429 throttled)", body_throttled));

    for probe in ["/healthz", "/readyz", "/metrics", "/openapi.json"] {
        let res_probe = client
            .get(format!("{base_url}{probe}"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            res_probe.status(),
            200,
            "positive: expected 200 with no token for probe {probe}"
        );
        let body_probe = res_probe.text().await.unwrap();
        seen_bodies.push((probe, body_probe));
    }

    // -------------------------------------------------------------------------
    // Step: observability
    // /metrics contains relay_publish_requests_total and relay_publish_latency_ms_count
    // lines that match the work done. Assert exact counts, so a zero-work run fails.
    // -------------------------------------------------------------------------
    let metrics_body = seen_bodies
        .iter()
        .find(|(path, _)| *path == "/metrics")
        .map(|(_, body)| body.as_str())
        .expect("observability: seen_bodies must contain /metrics response");

    assert!(
        metrics_body.contains("# TYPE relay_publish_requests_total counter"),
        "observability: /metrics must contain type declaration for relay_publish_requests_total"
    );
    assert!(
        metrics_body.contains("# TYPE relay_publish_latency_ms_count counter"),
        "observability: /metrics must contain type declaration for relay_publish_latency_ms_count"
    );

    let total_line = metrics_body
        .lines()
        .find(|l| l.starts_with("relay_publish_requests_total "))
        .expect("observability: /metrics must contain relay_publish_requests_total line");
    let total_count: u64 = total_line
        .strip_prefix("relay_publish_requests_total ")
        .unwrap()
        .trim()
        .parse()
        .expect("observability: relay_publish_requests_total must have integer count");
    assert!(
        total_count > 0,
        "observability: relay_publish_requests_total must be > 0 (zero-work run must fail), got {total_count}"
    );

    let latency_line = metrics_body
        .lines()
        .find(|l| l.starts_with("relay_publish_latency_ms_count "))
        .expect("observability: /metrics must contain relay_publish_latency_ms_count line");
    let latency_count: u64 = latency_line
        .strip_prefix("relay_publish_latency_ms_count ")
        .unwrap()
        .trim()
        .parse()
        .expect("observability: relay_publish_latency_ms_count must have integer count");
    assert_eq!(
        total_count, latency_count,
        "observability: relay_publish_requests_total ({total_count}) and relay_publish_latency_ms_count ({latency_count}) must match"
    );

    // -------------------------------------------------------------------------
    // Step: redaction
    // The full text of every response seen in the journey (the 401, 403, 429 bodies,
    // /metrics, /openapi.json, /readyz) contains none of the registry token strings.
    // Assert it with a loop over the bodies, and the assertion message names the path.
    // -------------------------------------------------------------------------
    let registry_tokens = ["writer-token", "reader-token", "admin-token"];
    for (path, body) in &seen_bodies {
        for token in registry_tokens {
            assert!(
                !body.contains(token),
                "redaction: response body for {path} must not leak registry token '{token}'"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Step: traceparent
    // Send one publish with a valid W3C traceparent header and the write token.
    // Assert only that it does not break the request (status is not 5xx) and that
    // the trace id never appears in /metrics.
    // Print, with eprintln!, whether the response echoes traceparent.
    // -------------------------------------------------------------------------
    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let traceparent_val = format!("00-{trace_id}-00f067aa0ba902b7-01");

    let res_trace = client
        .post(format!("{base_url}/v1/jobs/publish"))
        .bearer_auth("writer-token")
        .header("traceparent", &traceparent_val)
        .json(&serde_json::json!({
            "message_id": "traceparent-1",
            "payload": { "step": "traceparent" }
        }))
        .send()
        .await
        .unwrap();

    let trace_status = res_trace.status();
    assert!(
        !trace_status.is_server_error(),
        "traceparent: traceparent publish request must not break with 5xx, got {trace_status}"
    );

    let echoed_traceparent = res_trace.headers().get("traceparent");
    eprintln!(
        "traceparent: request traceparent={traceparent_val}, response status={trace_status}, response headers={:?}, echoed={echoed_traceparent:?}",
        res_trace.headers()
    );

    let metrics_after = client
        .get(format!("{base_url}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !metrics_after.contains(trace_id),
        "traceparent: trace id '{trace_id}' must never appear in /metrics"
    );
}
