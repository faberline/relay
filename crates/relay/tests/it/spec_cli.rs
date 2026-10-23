// HANDWRITE-BEGIN gap="missing-generator:unit-test:be8fe7b3" tracker="pending-tracker" reason="Drives the COMPILED relay binary offline (deploy_cli.rs harness pattern): spec parses as OpenAPI JSON listing the /v1 paths + /admin/backup; --format openapi-yaml parses via serde_yaml; --format json-schema parses as JSON with a components key; spec gen writes a non-empty client per lang (ts asserts types.ts/client.ts/index.ts); llm operations names the new surfaces."
//! Offline `relay spec` surface driven against the COMPILED `relay` binary
//! (WI #1209, keep #777 pattern): every `--format` emits a parseable document
//! that matches the served `/openapi.json` inventory (including the new
//! `/admin/backup`), `spec gen` writes a non-empty typed client per language
//! through the shared `openapi-codegen`, and the `llm` operations topic
//! names the backup / peer-TLS / spec surfaces. No server, no network.

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

/// R1 / AC3: shared llm topic surface conformance (claim `shared-llm-topic-surface`, WI #4).
/// Drives the compiled relay binary offline to verify:
/// - `relay llm --topic operations` prints the same output as `relay llm operations`.
/// - `relay llm` and `relay llm outline` still print the outline.
/// - The markdown outline lists `http-api` and `operations`.
/// - `relay llm --format json` parses as JSON with `project == "relay"`, a non-empty `version`,
///   and topic ids `["http-api", "operations"]`.
/// - `relay llm --topic http-api --format json` parses with `topic == "http-api"` and a non-empty `body`.
/// - Giving both a positional topic and `--topic` is a clap usage error (nonzero exit), not a silent pick.
/// - `relay llm nope` exits nonzero and stderr contains `unknown llm topic` and `outline, http-api, operations`.
#[test]
fn shared_llm_topic_surface_conformance() {
    // 1. relay llm and relay llm outline both print the outline, listing http-api and operations.
    let bare = stdout(&["llm"]);
    let outline = stdout(&["llm", "outline"]);
    assert_eq!(bare, outline, "relay llm and relay llm outline must match");
    assert!(
        bare.contains("http-api"),
        "outline must list http-api topic"
    );
    assert!(
        bare.contains("operations"),
        "outline must list operations topic"
    );

    // 2. relay llm --topic operations prints the same output as relay llm operations.
    let ops_flag = stdout(&["llm", "--topic", "operations"]);
    let ops_pos = stdout(&["llm", "operations"]);
    assert_eq!(
        ops_flag, ops_pos,
        "relay llm --topic operations must match relay llm operations"
    );

    // 3. relay llm --format json parses as JSON with project == "relay", non-empty version,
    // and topic ids ["http-api", "operations"].
    let json_outline_str = stdout(&["llm", "--format", "json"]);
    let json_outline: serde_json::Value =
        serde_json::from_str(&json_outline_str).expect("relay llm --format json emits JSON");
    assert_eq!(json_outline["project"], "relay");
    assert!(
        json_outline["version"]
            .as_str()
            .map_or(false, |v| !v.is_empty()),
        "version must be non-empty"
    );
    let topics_arr = json_outline["topics"]
        .as_array()
        .expect("topics must be array");
    let topic_ids: Vec<&str> = topics_arr.iter().filter_map(|t| t["id"].as_str()).collect();
    assert_eq!(
        topic_ids,
        vec!["http-api", "operations"],
        "JSON outline topic ids must be [\"http-api\", \"operations\"]"
    );

    // 4. relay llm --topic http-api --format json parses with topic == "http-api" and a non-empty body.
    let json_topic_str = stdout(&["llm", "--topic", "http-api", "--format", "json"]);
    let json_topic: serde_json::Value = serde_json::from_str(&json_topic_str)
        .expect("relay llm --topic http-api --format json emits JSON");
    assert_eq!(json_topic["topic"], "http-api");
    assert!(
        json_topic["body"].as_str().map_or(false, |b| !b.is_empty()),
        "body must be non-empty"
    );

    // 5. Giving both a positional topic and --topic is a clap usage error (nonzero exit).
    let both = run(&["llm", "--topic", "operations", "operations"]);
    assert!(
        !both.status.success(),
        "giving both positional topic and --topic must fail"
    );

    let both2 = run(&["llm", "operations", "--topic", "operations"]);
    assert!(
        !both2.status.success(),
        "giving both positional topic and --topic must fail"
    );

    // 6. relay llm nope exits nonzero and stderr contains unknown llm topic and outline, http-api, operations.
    let nope = run(&["llm", "nope"]);
    assert!(!nope.status.success(), "relay llm nope must exit nonzero");
    let nope_err = String::from_utf8_lossy(&nope.stderr);
    assert!(
        nope_err.contains("unknown llm topic"),
        "stderr must contain 'unknown llm topic': {nope_err}"
    );
    assert!(
        nope_err.contains("outline, http-api, operations"),
        "stderr must contain 'outline, http-api, operations': {nope_err}"
    );
}
// HANDWRITE-END
