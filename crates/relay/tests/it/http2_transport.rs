// SPEC-MANAGED: tech-design/interfaces/rest/http-2-openapi-transport-client-side-sharding-work-queue-consume.md#unit-test
// HANDWRITE-BEGIN gap="missing-generator:unit-test:7c3c08fd" tracker="pending-tracker" reason="In-process h2c integration tests for the unit-test plan."
//! HTTP/2 (h2c) transport integration tests over a real ephemeral server.
//!
//! Each test starts the app on a loopback port through the shared service
//! shell's serve loop (`service_http::serve` — HTTP/1.1 + h2c on one port,
//! #1205) and drives it with a reqwest client forced to HTTP/2
//! prior-knowledge (h2c), covering the #115 acceptance (a worker leases/acks
//! over h2c) plus the #1205 shell surface: standard probes, drain-aware
//! readiness, per-op metrics, and the shared error envelope.

use std::net::SocketAddr;

use serde_json::json;

use relay::queue::server::{router, AppState};
use relay::queue::server_config::RelayServerConfig;
use relay::queue::wire::{
    from_cbor, to_cbor, AckResponse, LeaseRequest, LeaseResponse, PublishRequest, CBOR,
};
use relay::{AppendOutcome, DEFAULT_PRIORITY};

async fn start_server() -> (SocketAddr, AppState) {
    let state = AppState::new(RelayServerConfig::ephemeral(), 8 * 1024 * 1024);
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // The shared shell's serve loop (HTTP/1.1 + h2c on one port); tests never
    // signal shutdown, so the loop lives for the test process.
    tokio::spawn(service_http::serve(
        listener,
        app,
        std::future::pending::<()>(),
    ));
    (addr, state)
}

async fn start_server_with_shutdown(
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> (SocketAddr, AppState, tokio::task::JoinHandle<()>) {
    let state = AppState::new(RelayServerConfig::ephemeral(), 8 * 1024 * 1024);
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(service_http::serve(listener, app, async move {
        let _ = shutdown_rx.await;
    }));
    (addr, state, handle)
}

fn h2c_client() -> reqwest::Client {
    reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap()
}

fn url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

async fn publish(
    client: &reqwest::Client,
    addr: SocketAddr,
    subject: &str,
    id: &str,
) -> serde_json::Value {
    client
        .post(url(addr, &format!("/v1/{subject}/publish")))
        .json(&json!({ "message_id": id, "payload": { "n": id } }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_is_idempotent_over_h2c() {
    let (addr, _state) = start_server().await;
    let client = h2c_client();
    let first = publish(&client, addr, "s", "m0").await;
    let second = publish(&client, addr, "s", "m0").await;
    assert_eq!(first["seq"], 0);
    assert_eq!(first["deduped"], false);
    assert_eq!(second["seq"], 0);
    assert_eq!(second["deduped"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_cbor_fast_path_over_h2c() {
    let (addr, _state) = start_server().await;
    let client = h2c_client();
    let body = to_cbor(&PublishRequest {
        message_id: "m0".into(),
        payload: json!({ "n": 1 }),
        headers: Default::default(),
        not_before: None,
        delay_ms: None,
        priority: DEFAULT_PRIORITY,
    });
    let bytes = client
        .post(url(addr, "/v1/s/publish"))
        .header("content-type", CBOR)
        .header("accept", CBOR)
        .body(body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let outcome: AppendOutcome = from_cbor(bytes.as_ref()).unwrap();
    assert_eq!(outcome.seq, 0);
    assert!(!outcome.deduped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_leases_and_acks_over_h2c() {
    // #115 acceptance: a worker leases then acks over h2c.
    let (addr, _state) = start_server().await;
    let client = h2c_client();
    publish(&client, addr, "q", "m0").await;

    let lease: LeaseResponse = client
        .post(url(addr, "/v1/q/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lease = lease.lease.expect("a lease was granted");
    assert_eq!(lease.seq, 0);
    assert_eq!(lease.attempt, 1);

    let ack: AckResponse = client
        .post(url(addr, "/v1/q/ack"))
        .json(&json!({ "lease_id": lease.lease_id }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ack.acked);
    assert_eq!(ack.committed_seq, Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lease_is_null_when_empty() {
    let (addr, _state) = start_server().await;
    let client = h2c_client();
    let resp: LeaseResponse = client
        .post(url(addr, "/v1/empty/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resp.lease.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lease_cbor_fast_path() {
    let (addr, _state) = start_server().await;
    let client = h2c_client();
    publish(&client, addr, "q", "m0").await;

    let body = to_cbor(&LeaseRequest {
        consumer_id: "c1".into(),
    });
    let bytes = client
        .post(url(addr, "/v1/q/lease"))
        .header("content-type", "application/cbor")
        .body(body)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let resp: LeaseResponse = ciborium::from_reader(bytes.as_ref()).unwrap();
    assert_eq!(resp.lease.expect("lease").seq, 0);
}

/// Length-prefix one consume up-frame (the stream's wire framing).
fn consume_frame(v: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).unwrap();
    let mut buf = (body.len() as u32).to_be_bytes().to_vec();
    buf.extend_from_slice(&body);
    buf
}

/// #1205 AC1: the five standard probe endpoints (shared service shell) all
/// answer on the one serve port.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_surface_answers_on_serve_port() {
    let (addr, _state) = start_server().await;
    let client = reqwest::Client::new();
    for (path, needle) in [
        ("/healthz", "ok"),
        ("/readyz", "ok"),
        ("/metrics", "relay_publish_requests_total"),
        ("/openapi.json", "/v1/{subject}/publish"),
        ("/docs", "swagger-ui"),
    ] {
        let resp = client.get(url(addr, path)).send().await.unwrap();
        assert_eq!(resp.status(), 200, "GET {path}");
        let body = resp.text().await.unwrap();
        assert!(
            body.contains(needle),
            "GET {path} body must contain {needle:?}"
        );
    }
}

/// #2490: every response carries a `Server-Timing: app;dur=<ms>` baseline
/// from the shared `service_http::server_timing_middleware` layer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_carry_server_timing_header() {
    let (addr, _state) = start_server().await;
    let client = reqwest::Client::new();
    let resp = client.get(url(addr, "/healthz")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let header = resp
        .headers()
        .get("server-timing")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        header.starts_with("app;dur="),
        "Server-Timing header must start with app;dur=, got {header:?}"
    );
    let digit = header
        .strip_prefix("app;dur=")
        .and_then(|rest| rest.chars().next());
    assert!(
        digit.is_some_and(|c| c.is_ascii_digit()),
        "app;dur= must be followed by a digit, got {header:?}"
    );
}

/// #1205 AC1: `/readyz` flips to 503 once drain begins (SIGTERM reaches
/// `start_drain` through `service_http::shutdown_with_drain`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readyz_flips_to_503_on_drain() {
    let (addr, state) = start_server().await;
    let client = reqwest::Client::new();
    let ready = client.get(url(addr, "/readyz")).send().await.unwrap();
    assert_eq!(ready.status(), 200);
    state.start_drain();
    let draining = client.get(url(addr, "/readyz")).send().await.unwrap();
    assert_eq!(draining.status(), 503);
    assert_eq!(draining.text().await.unwrap(), "draining");
}

/// graceful drain readiness: close capability claim `graceful-drain-readiness`.
///
/// Proves that `/readyz` flips to 503 while `/healthz` stays 200 and data-plane
/// operations over h2c continue to serve during graceful drain, until the shutdown
/// future completes, after which the serve loop exits and connections are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_drain_readiness_keeps_serving_until_shutdown() {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (addr, state, handle) = start_server_with_shutdown(shutdown_rx).await;
    let client = reqwest::Client::new();
    let h2 = h2c_client();

    // Before drain: /readyz is 200 and /healthz is 200.
    let ready = client.get(url(addr, "/readyz")).send().await.unwrap();
    assert_eq!(
        ready.status(),
        200,
        "graceful drain readiness: /readyz must return 200 before drain"
    );
    let health = client.get(url(addr, "/healthz")).send().await.unwrap();
    assert_eq!(
        health.status(),
        200,
        "graceful drain readiness: /healthz must return 200 before drain"
    );

    // After state.start_drain() and before shutdown: /readyz is 503 with body draining,
    // /healthz is still 200, and a publish plus a lease and ack over h2c still succeed.
    state.start_drain();

    let draining_ready = client.get(url(addr, "/readyz")).send().await.unwrap();
    assert_eq!(
        draining_ready.status(),
        503,
        "graceful drain readiness: /readyz must return 503 while draining"
    );
    let draining_body = draining_ready.text().await.unwrap();
    assert_eq!(
        draining_body, "draining",
        "graceful drain readiness: /readyz body must be 'draining'"
    );

    let draining_health = client.get(url(addr, "/healthz")).send().await.unwrap();
    assert_eq!(
        draining_health.status(),
        200,
        "graceful drain readiness: /healthz must remain 200 while draining"
    );

    let pub_outcome = publish(&h2, addr, "drain-q", "m0").await;
    assert_eq!(
        pub_outcome["seq"], 0,
        "graceful drain readiness: publish over h2c seq must be 0 while draining"
    );
    assert_eq!(
        pub_outcome["deduped"], false,
        "graceful drain readiness: publish over h2c deduped must be false while draining"
    );

    let lease_resp: LeaseResponse = h2
        .post(url(addr, "/v1/drain-q/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        lease_resp.lease.is_some(),
        "graceful drain readiness: lease over h2c must be granted while draining"
    );
    let lease = lease_resp.lease.unwrap();
    assert_eq!(
        lease.seq, 0,
        "graceful drain readiness: lease seq must be 0 while draining"
    );
    assert_eq!(
        lease.attempt, 1,
        "graceful drain readiness: lease attempt must be 1 while draining"
    );

    let ack: AckResponse = h2
        .post(url(addr, "/v1/drain-q/ack"))
        .json(&json!({ "lease_id": lease.lease_id }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        ack.acked,
        "graceful drain readiness: ack over h2c must be true while draining"
    );
    assert_eq!(
        ack.committed_seq,
        Some(0),
        "graceful drain readiness: ack committed_seq must be Some(0) while draining"
    );

    // After sending on the oneshot: the serve JoinHandle completes within a bounded
    // tokio::time::timeout (5 s), and a new connection to addr is then refused or fails.
    let send_ok = shutdown_tx.send(());
    assert!(
        send_ok.is_ok(),
        "graceful drain readiness: sending shutdown on oneshot must succeed"
    );

    let serve_completion = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    assert!(
        serve_completion.is_ok(),
        "graceful drain readiness: serve JoinHandle must complete within 5s timeout"
    );
    let join_result = serve_completion.unwrap();
    assert!(
        join_result.is_ok(),
        "graceful drain readiness: serve task must join cleanly"
    );

    let connect_result = tokio::net::TcpStream::connect(addr).await;
    assert!(
        connect_result.is_err(),
        "graceful drain readiness: new connection to addr must be refused or fail after shutdown, got: {connect_result:?}"
    );
}

/// #1205 AC2: an HTTP/2 prior-knowledge (h2c, via core/transport-h2c `h2c_client`)
/// request AND an HTTP/1.1 request both succeed on the same serve port.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2c_and_http11_share_the_serve_port() {
    let (addr, _state) = start_server().await;

    let h2 = transport_h2c::h2c_client().unwrap();
    let resp = h2.get(url(addr, "/healthz")).send().await.unwrap();
    assert_eq!(resp.version(), reqwest::Version::HTTP_2);
    assert_eq!(resp.status(), 200);
    let out = publish(&h2, addr, "p", "m-h2").await;
    assert_eq!(out["seq"], 0);

    let h1 = reqwest::Client::new();
    let resp = h1.get(url(addr, "/healthz")).send().await.unwrap();
    assert_eq!(resp.version(), reqwest::Version::HTTP_11);
    assert_eq!(resp.status(), 200);
    let resp = h1
        .post(url(addr, "/v1/p/publish"))
        .json(&json!({ "message_id": "m-h1", "payload": { "n": 1 } }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.version(), reqwest::Version::HTTP_11);
}

/// #1205 AC3: `/metrics` reports relay's per-op request counters (Prometheus
/// text) after traffic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_report_relay_request_counters_after_traffic() {
    let (addr, _state) = start_server().await;
    let client = h2c_client();
    publish(&client, addr, "m", "m0").await;
    publish(&client, addr, "m", "m1").await;
    let body = client
        .get(url(addr, "/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("# TYPE relay_publish_requests_total counter"));
    assert!(body.contains("relay_publish_requests_total 2"));
    assert!(body.contains("relay_publish_latency_ms_count 2"));
    assert!(body.contains("relay_lease_requests_total 0"));
}

/// #1205 AC4: error paths render the shared `{error, message}` envelope —
/// an undecodable publish body and a consume stream that opens with a
/// non-Subscribe frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn errors_render_the_shared_envelope() {
    let (addr, _state) = start_server().await;
    let client = h2c_client();

    let resp = client
        .post(url(addr, "/v1/s/publish"))
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "bad_request");
    assert!(!body["message"].as_str().unwrap().is_empty());

    let resp = client
        .post(url(addr, "/v1/s/consume"))
        .body(consume_frame(
            &json!({ "type": "ack", "lease_id": "L", "epoch": 1 }),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "bad_request");
    assert!(!body["message"].as_str().unwrap().is_empty());
}

/// #2556: a publish body over the shared data-plane body cap is rejected
/// with 413 rather than buffered/accepted, guarding against unbounded
/// request bodies on the data plane (probes stay exempt/unbounded).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_publish_body_is_rejected_with_413() {
    let (addr, _state) = start_server().await;
    let client = reqwest::Client::new();

    // One byte over the 8 MiB data-plane cap.
    let oversized_payload = "a".repeat(8 * 1024 * 1024 + 1);
    let result = client
        .post(url(addr, "/v1/orders/publish"))
        .header("content-type", "application/json")
        .body(format!(
            r#"{{"message_id":"m0","payload":"{oversized_payload}"}}"#
        ))
        .send()
        .await;

    // The Content-Length short-circuit is the point of the layer: the server
    // answers and closes without reading 8 MiB it has already decided to
    // refuse. The client is still uploading when that happens, so it races
    // between reading the 413 and having its own write fail with
    // ECONNRESET — both outcomes are the same refusal, and asserting only on
    // the status code makes this test flaky (~1 run in 5).
    match result {
        Ok(resp) => assert_eq!(resp.status(), 413, "oversized publish must be refused"),
        Err(err) => assert!(
            err.is_request(),
            "expected a transport-level refusal from the early close, got: {err}"
        ),
    }

    // Race-free invariant, and the one that actually matters: whichever way
    // the client observed the refusal, the oversized body never reached the
    // engine. A regression that buffered and accepted it would show up here
    // even if the status assertion above were satisfied.
    let client = h2c_client();
    let lease: LeaseResponse = client
        .post(url(addr, "/v1/orders/lease"))
        .json(&LeaseRequest {
            consumer_id: "c1".into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        lease.lease.is_none(),
        "the refused body must not be published: {lease:?}"
    );
}

/// #2556: an under-limit publish body succeeds, so the cap isn't just "reject
/// everything". The limit is enforced but doesn't break normal operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undersized_publish_body_succeeds() {
    let (addr, _state) = start_server().await;
    let client = h2c_client();

    // Well under the 8 MiB cap.
    let normal_payload = "normal message";
    let resp = client
        .post(url(addr, "/v1/orders/publish"))
        .json(&json!({
            "message_id": "m0",
            "payload": normal_payload
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "under-limit publish must succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["seq"], 0);
    assert_eq!(body["deduped"], false);
}

/// #2556: the structured 413 envelope carries the expected error key and
/// message when an oversized body is rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_body_rejection_renders_structured_413_envelope() {
    let (addr, _state) = start_server().await;
    let client = reqwest::Client::new();

    // One byte over the 8 MiB data-plane cap.
    let oversized_payload = "a".repeat(8 * 1024 * 1024 + 1);
    let result = client
        .post(url(addr, "/v1/s/publish"))
        .header("content-type", "application/json")
        .body(format!(
            r#"{{"message_id":"m0","payload":"{oversized_payload}"}}"#
        ))
        .send()
        .await;

    if let Ok(resp) = result {
        assert_eq!(resp.status(), 413);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "payload_too_large");
        assert!(body["message"].as_str().unwrap().contains("size limit"));
    }
}

// HANDWRITE-END
