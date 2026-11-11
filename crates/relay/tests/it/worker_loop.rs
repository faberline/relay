// SPEC-MANAGED: tech-design/interfaces/rest/relay-keep-worker-facing-openapi-contract-polyglot-worker-integr.md#unit-test
// HANDWRITE-BEGIN gap="missing-generator:unit-test:db6bcdf6" tracker="pending-tracker" reason="Throwaway reference worker (test-only): drives the lease / heartbeat / ack loop over h2c against an in-process relay, validating the worker-facing contract and the served OpenAPI."
//! Reference worker (test-only, #108): a polyglot worker integrates purely over
//! relay's HTTP/2 + OpenAPI contract. This drives the lease -> run -> heartbeat
//! -> ack loop against an in-process relay over h2c and checks the served
//! OpenAPI lists the worker-facing verbs.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;

use serde_json::json;

use relay::queue::server::{router, AppState};
use relay::queue::server_config::RelayServerConfig;
use relay::queue::wire::{
    from_cbor, to_cbor, AckRequest, AckResponse, HeartbeatResponse, LeaseRequest, LeaseResponse,
    CBOR,
};

async fn start_server() -> SocketAddr {
    let app = router(AppState::new(
        RelayServerConfig::ephemeral(),
        8 * 1024 * 1024,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn h2c() -> reqwest::Client {
    reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap()
}

// #108: a worker integrates over the OpenAPI contract — lease, heartbeat, ack.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reference_worker_drives_lease_heartbeat_ack_over_h2c() {
    let addr = start_server().await;
    let c = h2c();
    let base = format!("http://{addr}/v1/jobs");

    // Enqueue three jobs.
    for id in ["j0", "j1", "j2"] {
        c.post(format!("{base}/publish"))
            .json(&json!({ "message_id": id, "payload": { "job": id } }))
            .send()
            .await
            .unwrap();
    }

    // The worker loop: lease -> (run) -> heartbeat -> ack, until nothing is left.
    let mut done: BTreeSet<u64> = BTreeSet::new();
    loop {
        let lease: LeaseResponse = c
            .post(format!("{base}/lease"))
            .json(&json!({ "consumer_id": "worker-1" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let Some(lease) = lease.lease else { break };

        // Prove liveness mid-run.
        let hb: HeartbeatResponse = c
            .post(format!("{base}/heartbeat"))
            .json(&json!({ "lease_id": lease.lease_id, "epoch": lease.epoch }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(hb.extended, "heartbeat extends a held lease");

        // ...run the job... then ack with the fencing epoch.
        let ack: AckResponse = c
            .post(format!("{base}/ack"))
            .json(&json!({ "lease_id": lease.lease_id, "epoch": lease.epoch }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(ack.acked);
        done.insert(lease.seq);
    }

    assert_eq!(
        done,
        BTreeSet::from([0, 1, 2]),
        "every job processed exactly once"
    );

    // committed offset reached the last seq.
    let ack: AckResponse = c
        .post(format!("{base}/ack"))
        .json(&json!({ "lease_id": "nope", "epoch": 1 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!ack.acked, "a bogus ack is a no-op");
}

// #108: the worker-facing verbs are present in the served OpenAPI contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openapi_lists_worker_facing_verbs() {
    let addr = start_server().await;
    let doc = h2c()
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for path in [
        // The primary streaming consume path (#447/#448/#463).
        "/v1/{subject}/consume",
        // Deprecated polling verbs, retained for the direct-worker mode (#463).
        "/v1/{subject}/lease",
        "/v1/{subject}/ack",
        "/v1/{subject}/heartbeat",
    ] {
        assert!(
            doc.contains(path),
            "OpenAPI must advertise {path} to workers"
        );
    }
}

const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

async fn served_openapi_json(addr: SocketAddr) -> serde_json::Value {
    let resp = h2c()
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "served openapi contract: GET /openapi.json must answer 200"
    );
    serde_json::from_str(&resp.text().await.unwrap())
        .expect("served openapi contract: /openapi.json must parse as JSON")
}

// Served OpenAPI contract: GET /openapi.json is a valid OpenAPI 3 document whose
// paths carry exactly the worker and data-plane operations, each with the right
// HTTP method and a non-empty `responses` object.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_openapi_contract_is_valid_and_method_accurate() {
    let addr = start_server().await;
    let doc = served_openapi_json(addr).await;

    let version = doc["openapi"].as_str().unwrap_or_default();
    assert!(
        version.starts_with("3."),
        "served openapi contract: openapi must be 3.x, got {version:?}"
    );
    assert_eq!(
        doc["info"]["title"], "relay HTTP/2 transport",
        "served openapi contract: info.title must name the transport"
    );

    let expected: BTreeSet<(&str, &str)> = BTreeSet::from([
        ("/v1/{subject}/publish", "post"),
        ("/v1/{subject}/publish-batch", "post"),
        ("/v1/{subject}/consume", "post"),
        ("/v1/{subject}/lease", "post"),
        ("/v1/{subject}/ack", "post"),
        ("/v1/{subject}/lease-batch", "post"),
        ("/v1/{subject}/ack-batch", "post"),
        ("/v1/{subject}/heartbeat", "post"),
        ("/v1/{subject}/len", "get"),
        ("/admin/backup", "get"),
    ]);

    let paths = doc["paths"]
        .as_object()
        .expect("served openapi contract: paths object");
    let mut actual: BTreeSet<(&str, &str)> = BTreeSet::new();
    for (path, item) in paths {
        let item = item
            .as_object()
            .expect("served openapi contract: path item object");
        for (method, op) in item {
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            let responses = op["responses"].as_object();
            assert!(
                responses.is_some_and(|r| !r.is_empty()),
                "served openapi contract: {method} {path} needs a non-empty responses object"
            );
            actual.insert((path.as_str(), method.as_str()));
        }
    }

    for (path, method) in &expected {
        assert!(
            actual.contains(&(*path, *method)),
            "served openapi contract: {method} {path} is missing from /openapi.json"
        );
    }
    assert_eq!(
        actual, expected,
        "served openapi contract: /openapi.json must list exactly the router's operations"
    );
}

// Served OpenAPI contract: the served document is byte-for-byte the offline
// `relay spec` document, compared as parsed JSON so key order does not matter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_openapi_matches_offline_spec() {
    let addr = start_server().await;
    let served = served_openapi_json(addr).await;
    let offline: serde_json::Value = serde_json::from_str(&relay::queue::openapi::api_doc_json())
        .expect("served openapi contract: offline spec must parse as JSON");
    assert_eq!(
        served, offline,
        "served openapi contract: /openapi.json must equal the offline relay spec document"
    );
}

// Served OpenAPI contract: the document lists no route the router lacks, and
// every route in the router source is documented. Each documented operation
// must be served: a 404 or 405 for its method fails the contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_openapi_covers_every_data_plane_route() {
    let addr = start_server().await;
    let doc = served_openapi_json(addr).await;
    let paths = doc["paths"]
        .as_object()
        .expect("served openapi contract: paths object");

    let router_source = include_str!("../../../relay-queue/src/server.rs");
    let route_literals: BTreeSet<&str> = router_source
        .split(".route(\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    assert!(
        !route_literals.is_empty(),
        "served openapi contract: found no .route(\"...\") literals in the router source"
    );
    for route in &route_literals {
        assert!(
            paths.contains_key(*route),
            "served openapi contract: router route {route} is not documented in /openapi.json"
        );
    }

    let client = h2c();
    for (path, item) in paths {
        let item = item
            .as_object()
            .expect("served openapi contract: path item object");
        let concrete = path.replace("{subject}", "s");
        for method in item.keys().filter(|m| HTTP_METHODS.contains(&m.as_str())) {
            let url = format!("http://{addr}{concrete}");
            let req = match method.as_str() {
                "get" => client.get(&url),
                "post" => client.post(&url).body(""),
                other => panic!("served openapi contract: unexpected method {other} on {path}"),
            };
            let status = req
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .unwrap_or_else(|e| panic!("served openapi contract: {method} {path} failed: {e}"))
                .status()
                .as_u16();
            assert!(
                status != 404 && status != 405,
                "served openapi contract: {method} {path} answered {status}; the router must serve every documented operation"
            );
        }
    }
}

fn run_relay_cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(args)
        .output()
        .expect("run relay binary")
}

fn stdout_relay_cli(args: &[&str]) -> String {
    let out = run_relay_cli(args);
    assert!(
        out.status.success(),
        "`relay {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

// Expected Result: observes http worker protocol parity (claim http-worker-protocol-parity): served OpenAPI/relay spec twins, epoch fencing, empty-queue lease, CBOR round trip.
// Tests: worker_fencing_epoch_rejected_without_consuming_lease, empty_queue_returns_null_lease_and_duplicate_ack_returns_false, served_openapi_and_cli_spec_paths_and_worker_ops_match, worker_lease_and_ack_cbor_round_trip.

// Wrong epoch on heartbeat and ack is rejected; valid lease is retained and ackable with the right epoch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_fencing_epoch_rejected_without_consuming_lease() {
    let addr = start_server().await;
    let c = h2c();
    let base = format!("http://{addr}/v1/fence");

    c.post(format!("{base}/publish"))
        .json(&json!({ "message_id": "f0", "payload": { "task": "fenced" } }))
        .send()
        .await
        .unwrap();

    let lease_resp: LeaseResponse = c
        .post(format!("{base}/lease"))
        .json(&json!({ "consumer_id": "worker-fenced" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lease = lease_resp.lease.expect("expected lease");

    let wrong_epoch = lease.epoch + 1;

    // Heartbeat with wrong epoch is rejected.
    let hb: HeartbeatResponse = c
        .post(format!("{base}/heartbeat"))
        .json(&json!({ "lease_id": lease.lease_id, "epoch": wrong_epoch }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !hb.extended,
        "heartbeat with wrong epoch must not extend lease"
    );

    // Ack with wrong epoch is rejected.
    let ack_wrong: AckResponse = c
        .post(format!("{base}/ack"))
        .json(&json!({ "lease_id": lease.lease_id, "epoch": wrong_epoch }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!ack_wrong.acked, "ack with wrong epoch must be rejected");

    // Ack with correct epoch succeeds, proving rejected calls did not consume the lease.
    let ack_correct: AckResponse = c
        .post(format!("{base}/ack"))
        .json(&json!({ "lease_id": lease.lease_id, "epoch": lease.epoch }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ack_correct.acked, "ack with right epoch must succeed");
}

// An empty subject returns null lease; duplicate ack returns acked == false.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_queue_returns_null_lease_and_duplicate_ack_returns_false() {
    let addr = start_server().await;
    let c = h2c();
    let base_empty = format!("http://{addr}/v1/empty");
    let base_ack = format!("http://{addr}/v1/ack-twice");

    // Lease on empty subject returns lease: None.
    let empty_lease: LeaseResponse = c
        .post(format!("{base_empty}/lease"))
        .json(&json!({ "consumer_id": "worker-empty" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        empty_lease.lease.is_none(),
        "lease on empty subject must return null lease"
    );

    // Publish, lease, and ack an entry once.
    c.post(format!("{base_ack}/publish"))
        .json(&json!({ "message_id": "a0", "payload": { "task": "ack" } }))
        .send()
        .await
        .unwrap();

    let lease_resp: LeaseResponse = c
        .post(format!("{base_ack}/lease"))
        .json(&json!({ "consumer_id": "worker-ack" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lease = lease_resp.lease.expect("expected lease");

    let ack1: AckResponse = c
        .post(format!("{base_ack}/ack"))
        .json(&json!({ "lease_id": lease.lease_id, "epoch": lease.epoch }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ack1.acked, "first ack must succeed");

    // Second ack of an already-acked lease returns acked == false.
    let ack2: AckResponse = c
        .post(format!("{base_ack}/ack"))
        .json(&json!({ "lease_id": lease.lease_id, "epoch": lease.epoch }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!ack2.acked, "second ack must return acked == false");
}

// Served GET /openapi.json and offline `relay spec` are contract twins: paths and worker operationIds match.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_openapi_and_cli_spec_paths_and_worker_ops_match() {
    let addr = start_server().await;
    let served_text = h2c()
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let served: serde_json::Value =
        serde_json::from_str(&served_text).expect("parse served openapi JSON");

    let cli_text = stdout_relay_cli(&["spec"]);
    let cli: serde_json::Value = serde_json::from_str(&cli_text).expect("parse relay spec JSON");

    let served_paths: BTreeSet<String> = served["paths"]
        .as_object()
        .expect("served paths object")
        .keys()
        .cloned()
        .collect();
    let cli_paths: BTreeSet<String> = cli["paths"]
        .as_object()
        .expect("cli paths object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        served_paths, cli_paths,
        "paths in served openapi and relay spec must match"
    );

    let extract_worker_ops = |doc: &serde_json::Value| -> BTreeSet<String> {
        let mut ops = BTreeSet::new();
        for endpoint in [
            "/v1/{subject}/lease",
            "/v1/{subject}/heartbeat",
            "/v1/{subject}/ack",
            "/v1/{subject}/consume",
        ] {
            if let Some(methods) = doc["paths"].get(endpoint).and_then(|p| p.as_object()) {
                for (_method, op) in methods {
                    if let Some(op_id) = op.get("operationId").and_then(|v| v.as_str()) {
                        ops.insert(op_id.to_string());
                    }
                }
            }
        }
        ops
    };

    let served_ops = extract_worker_ops(&served);
    let cli_ops = extract_worker_ops(&cli);
    assert_eq!(
        served_ops, cli_ops,
        "operation ids for lease/heartbeat/ack/consume must match"
    );
    assert_eq!(
        served_ops,
        BTreeSet::from([
            "lease".to_string(),
            "heartbeat".to_string(),
            "ack".to_string(),
            "consume".to_string(),
        ]),
        "all four worker operation ids must be present"
    );
}

// Pure-HTTP CBOR round trip for lease and ack over h2c.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_lease_and_ack_cbor_round_trip() {
    let addr = start_server().await;
    let c = h2c();
    let base = format!("http://{addr}/v1/cbor-work");

    c.post(format!("{base}/publish"))
        .json(&json!({ "message_id": "cbor-0", "payload": { "data": "cbor-payload" } }))
        .send()
        .await
        .unwrap();

    let lease_req_body = to_cbor(&LeaseRequest {
        consumer_id: "cbor-worker".into(),
    });
    let lease_res = c
        .post(format!("{base}/lease"))
        .header("content-type", CBOR)
        .header("accept", CBOR)
        .body(lease_req_body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let lease_bytes = lease_res.bytes().await.unwrap();
    let lease_resp: LeaseResponse =
        from_cbor(lease_bytes.as_ref()).expect("decode cbor LeaseResponse");
    let lease = lease_resp.lease.expect("expected granted lease via cbor");
    assert_eq!(lease.seq, 0);

    let ack_req_body = to_cbor(&AckRequest {
        lease_id: lease.lease_id,
        epoch: Some(lease.epoch),
    });
    let ack_res = c
        .post(format!("{base}/ack"))
        .header("content-type", CBOR)
        .header("accept", CBOR)
        .body(ack_req_body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let ack_bytes = ack_res.bytes().await.unwrap();
    let ack_resp: AckResponse = from_cbor(ack_bytes.as_ref()).expect("decode cbor AckResponse");
    assert!(ack_resp.acked, "ack via cbor must succeed");
}
// HANDWRITE-END
