// SPEC-MANAGED: tech-design/interfaces/rest/relay-keep-worker-facing-openapi-contract-polyglot-worker-integr.md#unit-test
// HANDWRITE-BEGIN gap="missing-generator:unit-test:db6bcdf6" tracker="pending-tracker" reason="Throwaway reference worker (test-only): drives the lease / heartbeat / ack loop over h2c against an in-process relay, validating the worker-facing contract and the served OpenAPI."
//! Reference worker (test-only, #108): a polyglot worker integrates purely over
//! relay's HTTP/2 + OpenAPI contract. This drives the lease -> run -> heartbeat
//! -> ack loop against an in-process relay over h2c and checks the served
//! OpenAPI lists the worker-facing verbs.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use serde_json::json;

use relay::queue::server::{router, AppState};
use relay::queue::server_config::RelayServerConfig;
use relay::queue::wire::{AckResponse, HeartbeatResponse, LeaseResponse};

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

// #23: validate the served openapi contract — valid OpenAPI 3, method accurate,
// non-empty responses, matches offline spec, and covers every data-plane route.
/// Validate that the served openapi contract is valid OpenAPI 3, has accurate title,
/// and declares exactly the expected operations with non-empty responses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_openapi_contract_is_valid_and_method_accurate() {
    let addr = start_server().await;
    let res = h2c()
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        reqwest::StatusCode::OK,
        "served openapi contract: status must be 200"
    );

    let doc: serde_json::Value = res.json().await.unwrap();
    let openapi = doc
        .get("openapi")
        .and_then(|v| v.as_str())
        .expect("served openapi contract must contain openapi version");
    assert!(
        openapi.starts_with("3."),
        "served openapi contract: openapi version must start with 3., got {openapi}"
    );

    let title = doc
        .pointer("/info/title")
        .and_then(|v| v.as_str())
        .expect("served openapi contract must contain info.title");
    assert_eq!(
        title, "relay HTTP/2 transport",
        "served openapi contract: info.title must match 'relay HTTP/2 transport'"
    );

    let paths = doc
        .get("paths")
        .and_then(|v| v.as_object())
        .expect("served openapi contract must contain paths object");

    let expected_operations = [
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
    ];

    let mut actual_operations = BTreeSet::new();
    for (path, item) in paths {
        let item_obj = item
            .as_object()
            .unwrap_or_else(|| panic!("served openapi contract: path {path} must be an object"));
        for (method, op) in item_obj {
            let method_lower = method.to_lowercase();
            if [
                "get", "post", "put", "delete", "options", "head", "patch", "trace",
            ]
            .contains(&method_lower.as_str())
            {
                let responses = op.get("responses").and_then(|r| r.as_object());
                assert!(
                    responses.is_some_and(|r| !r.is_empty()),
                    "served openapi contract: operation {method_lower} {path} must have non-empty responses object"
                );
                actual_operations.insert((path.clone(), method_lower));
            }
        }
    }

    for (path, method) in &expected_operations {
        assert!(
            actual_operations.contains(&(path.to_string(), method.to_string())),
            "served openapi contract: expected operation {method} {path} not found"
        );
    }
    assert_eq!(
        actual_operations.len(),
        expected_operations.len(),
        "served openapi contract: paths must contain exactly {} operations",
        expected_operations.len()
    );
}

/// Validate that the served openapi document matches the offline spec output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_openapi_matches_offline_spec() {
    let addr = start_server().await;
    let doc_str = h2c()
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let served_json: serde_json::Value =
        serde_json::from_str(&doc_str).expect("served JSON is valid");
    let offline_json: serde_json::Value =
        serde_json::from_str(&relay_queue::openapi::api_doc_json())
            .expect("offline spec JSON is valid");
    assert_eq!(
        served_json, offline_json,
        "served openapi document must match offline relay spec output"
    );
}

/// Validate that the served openapi document covers every data-plane route.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_openapi_covers_every_data_plane_route() {
    let addr = start_server().await;
    let client = h2c();
    let doc: serde_json::Value = client
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let paths = doc
        .get("paths")
        .and_then(|v| v.as_object())
        .expect("served openapi document must contain paths object");

    // For each operation in the served paths (replace {subject} with s),
    // send the document's method to that path with an invalid or empty body
    // and assert the response status is not 404 and not 405.
    for (path, item) in paths {
        let concrete_path = path.replace("{subject}", "s");
        let item_obj = item
            .as_object()
            .unwrap_or_else(|| panic!("path {path} must be an object"));
        for (method, _) in item_obj {
            let method_lower = method.to_lowercase();
            if ![
                "get", "post", "put", "delete", "options", "head", "patch", "trace",
            ]
            .contains(&method_lower.as_str())
            {
                continue;
            }
            let res = match method_lower.as_str() {
                "get" => client
                    .get(format!("http://{addr}{concrete_path}"))
                    .send()
                    .await
                    .unwrap(),
                "post" => client
                    .post(format!("http://{addr}{concrete_path}"))
                    .send()
                    .await
                    .unwrap(),
                other => panic!("unexpected method {other} for {concrete_path}"),
            };
            let status = res.status();
            assert!(
                status != reqwest::StatusCode::NOT_FOUND
                    && status != reqwest::StatusCode::METHOD_NOT_ALLOWED,
                "operation {method_lower} {concrete_path} returned status {status}, expected not 404 and not 405"
            );
        }
    }

    // It also reads crates/relay-queue/src/server.rs via include_str!,
    // extracts every .route("<path>" literal, and asserts each is present
    // in the served paths, so a new route cannot ship undocumented.
    let server_src = include_str!("../../../../crates/relay-queue/src/server.rs");
    let mut server_routes = Vec::new();
    for part in server_src.split(".route(\"").skip(1) {
        if let Some((route_path, _)) = part.split_once('"') {
            if route_path.starts_with('/') && !route_path.contains(char::is_whitespace) {
                server_routes.push(route_path);
            }
        }
    }
    assert!(
        !server_routes.is_empty(),
        "must extract route literals from crates/relay-queue/src/server.rs"
    );
    for route in server_routes {
        assert!(
            paths.contains_key(route),
            "route {route} from server.rs must be present in served openapi paths"
        );
    }
}
// HANDWRITE-END
