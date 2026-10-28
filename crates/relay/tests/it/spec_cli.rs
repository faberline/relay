// HANDWRITE-BEGIN gap="missing-generator:unit-test:be8fe7b3" tracker="pending-tracker" reason="Drives the COMPILED relay binary offline (deploy_cli.rs harness pattern): spec parses as OpenAPI JSON listing the /v1 paths + /admin/backup; --format openapi-yaml parses via serde_yaml; --format json-schema parses as JSON with a components key; spec gen writes a non-empty client per lang (ts asserts types.ts/client.ts/index.ts); llm operations names the new surfaces."
//! Offline `relay spec` surface driven against the COMPILED `relay` binary
//! (WI #1209, keep #777 pattern): every `--format` emits a parseable document
//! that matches the served `/openapi.json` inventory (including the new
//! `/admin/backup`), `spec gen` writes a non-empty typed client per language
//! through the shared `openapi-codegen`, and the `llm` operations topic
//! names the backup / peer-TLS / spec surfaces. No server, no network.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::process::Command;

use relay::queue::server::{router, AppState};
use relay::queue::server_config::RelayServerConfig;

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

/// R1 / AC1: `relay spec` emits parseable OpenAPI in every format — the
/// offline twin of `GET /openapi.json`, listing the /v1 data plane and the
/// new `/admin/backup`. `--format json-schema` carries the `components` key
/// honestly (relay registers no named schemas today, so it may be null —
/// never a faked catalog; keep's `--shapes`/`--fields` are deliberately
/// absent from relay's surface).
#[test]
fn spec_prints_parseable_openapi_in_every_format() {
    // Default: pretty OpenAPI JSON.
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&["spec"])).expect("`relay spec` emits JSON");
    assert!(doc["openapi"].is_string(), "openapi version field present");
    for path in [
        "/v1/{subject}/publish",
        "/v1/{subject}/consume",
        "/v1/{subject}/lease",
        "/v1/{subject}/len",
        "/admin/backup",
    ] {
        assert!(
            doc["paths"].get(path).is_some(),
            "OpenAPI doc must list {path}"
        );
    }

    // YAML for LLM/agent reading.
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&stdout(&["spec", "--format", "openapi-yaml"]))
            .expect("`relay spec --format openapi-yaml` emits YAML");
    assert!(
        yaml.get("openapi").is_some(),
        "YAML document carries the openapi version field"
    );

    // Component schemas only.
    let schema: serde_json::Value =
        serde_json::from_str(&stdout(&["spec", "--format", "json-schema"]))
            .expect("`relay spec --format json-schema` emits JSON");
    assert!(
        schema.get("components").is_some(),
        "json-schema view carries the components key (null is honest: relay \
         registers no named schemas)"
    );

    // relay has no request-shape/value catalogs: the keep-only flags must NOT
    // parse (omitted, not faked).
    assert!(
        !run(&["spec", "--shapes"]).status.success(),
        "--shapes is keep-only and must not parse on relay"
    );
    assert!(
        !run(&["spec", "--fields"]).status.success(),
        "--fields is keep-only and must not parse on relay"
    );
}

/// R1 / AC1: `relay spec gen --lang <l> --out <dir>` writes a non-empty
/// client and its pinned target manifest for every language via the shared
/// codegen; the TypeScript client carries the well-known entry files.
#[test]
fn spec_gen_writes_a_client_for_every_language() {
    for lang in ["ts", "py", "rust"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join(lang);
        let output = stdout(&[
            "spec",
            "gen",
            "--lang",
            lang,
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(output.lines().any(|line| line == "next: done"));
        let files: Vec<_> = std::fs::read_dir(&out)
            .expect("client output dir")
            .filter_map(|e| e.ok())
            .collect();
        assert!(!files.is_empty(), "{lang} client emitted files");
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.join(".openapi-codegen.json")).expect("target manifest"),
        )
        .expect("valid target manifest");
        let expected_target = match lang {
            "ts" => "typescript-5.0",
            "py" => "python-3.14",
            "rust" => "rust-2024",
            _ => unreachable!(),
        };
        assert_eq!(manifest["target"], expected_target, "{lang} policy target");
        for f in &files {
            assert!(
                f.metadata().expect("metadata").len() > 0,
                "{lang} client file {:?} is non-empty",
                f.file_name()
            );
        }
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("ts");
    let _ = stdout(&[
        "spec",
        "gen",
        "--lang",
        "ts",
        "--out",
        out.to_str().unwrap(),
    ]);
    for f in ["types.ts", "client.ts", "index.ts"] {
        assert!(out.join(f).is_file(), "generated {f}");
    }
}

/// An explicit target is an auditable one-off override of relay's policy.
#[test]
fn spec_gen_target_override_writes_the_requested_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("py311");
    let _ = stdout(&[
        "spec",
        "gen",
        "--lang",
        "py",
        "--target",
        "python-3.11",
        "--out",
        out.to_str().expect("utf-8 path"),
    ]);
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.join(".openapi-codegen.json")).expect("target manifest"),
    )
    .expect("valid target manifest");
    assert_eq!(manifest["target"], "python-3.11");
    assert_eq!(manifest["minimum_version"], "3.11");
}

/// R5: the `llm` operations topic documents the spec verbs, the backup verb +
/// endpoint + CronJob token env, and the peer-TLS env contract (with the
/// mTLS-termination seam gap stated, not hidden).
#[test]
fn llm_operations_topic_documents_the_new_surfaces() {
    let ops = stdout(&["llm", "operations"]);
    for needle in [
        "relay spec",
        "spec gen",
        "relay backup",
        "/admin/backup",
        "RELAY_BACKUP_TOKEN",
        "RELAY_PEER_TLS_CERT",
        "RELAY_PEER_MTLS",
    ] {
        assert!(
            ops.contains(needle),
            "llm operations must document {needle}"
        );
    }
}

async fn start_server() -> SocketAddr {
    let state = AppState::new(RelayServerConfig::ephemeral(), 8 * 1024 * 1024);
    let app = router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(service_http::serve(
        listener,
        app,
        std::future::pending::<()>(),
    ));
    addr
}

/// offline spec matches served contract
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_spec_matches_served_contract() {
    let addr = start_server().await;
    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .expect("build h2c client");
    let resp = client
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .expect("fetch /openapi.json");
    assert!(
        resp.status().is_success(),
        "/openapi.json responded with status {}",
        resp.status()
    );
    let served: serde_json::Value = resp
        .json()
        .await
        .expect("parse served /openapi.json as JSON");
    let offline: serde_json::Value =
        serde_json::from_str(&stdout(&["spec"])).expect("parse offline `relay spec` as JSON");

    assert_eq!(
        offline, served,
        "offline `relay spec` and served `/openapi.json` must be equal"
    );

    let expected_paths: BTreeSet<&str> = [
        "/v1/{subject}/publish",
        "/v1/{subject}/publish-batch",
        "/v1/{subject}/consume",
        "/v1/{subject}/lease",
        "/v1/{subject}/ack",
        "/v1/{subject}/lease-batch",
        "/v1/{subject}/ack-batch",
        "/v1/{subject}/heartbeat",
        "/v1/{subject}/len",
        "/admin/backup",
    ]
    .into_iter()
    .collect();

    let paths_map = served["paths"]
        .as_object()
        .expect("`paths` in /openapi.json must be a JSON object");
    let actual_paths: BTreeSet<&str> = paths_map.keys().map(String::as_str).collect();

    for path in &expected_paths {
        assert!(
            actual_paths.contains(path),
            "missing route in OpenAPI paths: {path}"
        );
    }
    for path in &actual_paths {
        assert!(
            expected_paths.contains(path),
            "unexpected route added to OpenAPI paths: {path}"
        );
    }
    assert_eq!(
        actual_paths, expected_paths,
        "set of keys of paths must equal expected route list"
    );
}

/// Assert that `--format openapi-yaml` and `--format json-schema` views match the default JSON document.
#[test]
fn offline_spec_yaml_and_json_schema_views_match_json() {
    let default_json: serde_json::Value =
        serde_json::from_str(&stdout(&["spec"])).expect("`relay spec` emits default JSON");

    let yaml_str = stdout(&["spec", "--format", "openapi-yaml"]);
    let yaml_as_json: serde_json::Value = serde_yaml::from_str(&yaml_str)
        .expect("parse `--format openapi-yaml` with serde_yaml as serde_json::Value");

    assert_eq!(
        yaml_as_json, default_json,
        "--format openapi-yaml must equal default JSON document"
    );

    let json_schema_doc: serde_json::Value =
        serde_json::from_str(&stdout(&["spec", "--format", "json-schema"]))
            .expect("`relay spec --format json-schema` emits JSON");

    let schema_components = json_schema_doc
        .get("components")
        .unwrap_or(&json_schema_doc);
    let default_components = default_json
        .get("components")
        .unwrap_or(&serde_json::Value::Null);

    let components_match = if schema_components.is_null() && default_components.is_null() {
        true
    } else {
        schema_components == default_components
    };

    assert!(
        components_match,
        "--format json-schema must equal components value of default JSON document"
    );
}
// HANDWRITE-END
