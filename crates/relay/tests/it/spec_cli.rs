// HANDWRITE-BEGIN gap="missing-generator:unit-test:be8fe7b3" tracker="pending-tracker" reason="Drives the COMPILED relay binary offline (deploy_cli.rs harness pattern): spec parses as OpenAPI JSON listing the /v1 paths + /admin/backup; --format openapi-yaml parses via serde_yaml; --format json-schema parses as JSON with a components key; spec gen writes a non-empty client per lang (ts asserts types.ts/client.ts/index.ts); llm operations names the new surfaces."
//! Offline `relay spec` surface driven against the COMPILED `relay` binary
//! (WI #1209, keep #777 pattern): every `--format` emits a parseable document
//! that matches the served `/openapi.json` inventory (including the new
//! `/admin/backup`), `spec gen` writes a non-empty typed client per language
//! through the shared `openapi-codegen`, and the `llm` operations topic
//! names the backup / peer-TLS / spec surfaces. No server, no network.

use std::collections::BTreeSet;
use std::process::Command;

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

pub(crate) fn relay_spec_doc() -> serde_json::Value {
    let raw = stdout(&["spec"]);
    serde_json::from_str(&raw).expect("`relay spec` emits JSON")
}

pub(crate) fn relay_spec_paths() -> BTreeSet<String> {
    let doc = relay_spec_doc();
    doc["paths"]
        .as_object()
        .expect("paths object in `relay spec` JSON")
        .keys()
        .cloned()
        .collect()
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

/// #24: `relay spec` lists the complete public API (all 10 endpoints),
/// matching YAML output and handler method declarations.
#[test]
fn spec_lists_all_ten_public_endpoints_with_accurate_methods() {
    let expected_paths = BTreeSet::from([
        "/admin/backup".to_string(),
        "/v1/{subject}/ack".to_string(),
        "/v1/{subject}/ack-batch".to_string(),
        "/v1/{subject}/consume".to_string(),
        "/v1/{subject}/heartbeat".to_string(),
        "/v1/{subject}/lease".to_string(),
        "/v1/{subject}/lease-batch".to_string(),
        "/v1/{subject}/len".to_string(),
        "/v1/{subject}/publish".to_string(),
        "/v1/{subject}/publish-batch".to_string(),
    ]);

    let doc = relay_spec_doc();
    let paths_obj = doc["paths"]
        .as_object()
        .expect("`relay spec` JSON must contain paths object");
    let actual_paths: BTreeSet<String> = paths_obj.keys().cloned().collect();

    let missing: Vec<_> = expected_paths.difference(&actual_paths).cloned().collect();
    let extra: Vec<_> = actual_paths.difference(&expected_paths).cloned().collect();
    assert!(
        actual_paths == expected_paths,
        "`relay spec` paths do not match expected 10 paths. Missing: {missing:?}, Extra: {extra:?}"
    );

    // The same test parses `relay spec --format openapi-yaml` and asserts its `paths` key set equals the JSON key set.
    let yaml_out = stdout(&["spec", "--format", "openapi-yaml"]);
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&yaml_out).expect("`relay spec --format openapi-yaml` emits YAML");
    let yaml_paths_obj = yaml
        .get("paths")
        .and_then(|v| v.as_mapping())
        .expect("`relay spec --format openapi-yaml` must contain paths mapping");
    let yaml_paths: BTreeSet<String> = yaml_paths_obj
        .keys()
        .filter_map(|k| k.as_str().map(|s| s.to_string()))
        .collect();
    assert_eq!(
        yaml_paths, actual_paths,
        "YAML paths key set must equal JSON paths key set"
    );

    // The test asserts each path lists the HTTP method its handler declares.
    // Methods declared via #[utoipa::path(...)]:
    // - publish, publish-batch, consume, lease, lease-batch, ack, ack-batch, heartbeat: post
    // - len, /admin/backup: get
    let expected_methods = [
        ("/v1/{subject}/publish", "post"),
        ("/v1/{subject}/publish-batch", "post"),
        ("/v1/{subject}/consume", "post"),
        ("/v1/{subject}/lease", "post"),
        ("/v1/{subject}/lease-batch", "post"),
        ("/v1/{subject}/ack", "post"),
        ("/v1/{subject}/ack-batch", "post"),
        ("/v1/{subject}/heartbeat", "post"),
        ("/v1/{subject}/len", "get"),
        ("/admin/backup", "get"),
    ];

    for (path, method) in expected_methods {
        let path_val = paths_obj
            .get(path)
            .unwrap_or_else(|| panic!("missing expected path {path} in JSON paths"));
        assert!(
            path_val.get(method).is_some(),
            "path {path} must declare HTTP method {method}"
        );
    }
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
// HANDWRITE-END
