// HANDWRITE-BEGIN gap="missing-generator:unit-test:22fdd793" tracker="pending-tracker" reason="Offline deploy-CLI tests driving the COMPILED relay binary in the default build: every k8s/dockerfile render verb succeeds and round-trips serde_yaml; dockerfile source/release outputs equal the committed fixtures (+ --version substitution); the CRD render is structural-schema safe (no uint32/uint64, minimum floor, kind Relay); operator run without the feature exits nonzero with the rebuild hint; the GKE test overlay runs the auto-mode env as three voters; the llm operations topic names the deploy verbs."
//! Offline deploy-CLI surface driven against the COMPILED `relay` binary in
//! the DEFAULT (kube-free) build (WI #1208): every `relay k8s ... render` and
//! `relay dockerfile render` verb succeeds offline and emits parseable
//! YAML/Dockerfiles; the committed Dockerfile fixtures are reproduced
//! byte-for-byte (keep #777 pattern); the CRD render is structural-schema
//! safe; the GKE test overlay is three-voter auto-mode; and the llm
//! operations topic names the deploy verbs. No server, no network.

use std::process::Command;

use serde::Deserialize;

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

/// Parse every YAML document in a (possibly multi-doc) stream, asserting the
/// stream is non-empty and each document is a mapping.
fn parse_yaml_docs(what: &str, yaml: &str) -> Vec<serde_yaml::Value> {
    let mut docs = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(yaml) {
        let value = serde_yaml::Value::deserialize(doc)
            .unwrap_or_else(|e| panic!("{what}: YAML document does not parse: {e}"));
        if !value.is_null() {
            assert!(value.is_mapping(), "{what}: document is not a mapping");
            docs.push(value);
        }
    }
    assert!(!docs.is_empty(), "{what}: no YAML documents emitted");
    docs
}

/// R4 / AC1: every render verb succeeds offline in the default build and
/// round-trips through serde_yaml; `operator run` without the feature exits
/// nonzero with the rebuild hint.
#[test]
fn render_verbs_emit_parseable_yaml_offline() {
    // CRD render: one CustomResourceDefinition document.
    let crd = parse_yaml_docs("crd render", &stdout(&["k8s", "crd", "render"]));
    assert_eq!(crd[0]["kind"], "CustomResourceDefinition");

    // Operator render: the control plane, namespace-substituted.
    let ops = parse_yaml_docs(
        "operator render",
        &stdout(&["k8s", "operator", "render", "--namespace", "relay-ops"]),
    );
    let kinds: Vec<&str> = ops.iter().filter_map(|d| d["kind"].as_str()).collect();
    for kind in [
        "Namespace",
        "ServiceAccount",
        "ClusterRole",
        "ClusterRoleBinding",
        "Deployment",
    ] {
        assert!(kinds.contains(&kind), "operator render missing {kind}");
    }
    let ns = ops.iter().find(|d| d["kind"] == "Namespace").unwrap();
    assert_eq!(ns["metadata"]["name"], "relay-ops", "namespace substituted");

    // Instance render: all four profiles emit a `kind: Relay` CR.
    for profile in ["dev", "staging", "prod", "template"] {
        let cr = parse_yaml_docs(
            &format!("instance render --profile {profile}"),
            &stdout(&["k8s", "instance", "render", "--profile", profile]),
        );
        assert_eq!(cr[0]["kind"], "Relay", "profile {profile}");
        assert_eq!(cr[0]["apiVersion"], "relay.dev/v1alpha1");
        assert!(cr[0]["spec"]["image"].is_string());
    }
    // The prod profile is the HA shape (the topology that used to live as
    // hand-maintained YAML in k8s/) and dogfoods the auth wiring.
    let prod: serde_yaml::Value =
        serde_yaml::from_str(&stdout(&["k8s", "instance", "render", "--profile", "prod"])).unwrap();
    assert_eq!(prod["spec"]["replicasPerShard"], serde_yaml::Value::from(3));
    assert_eq!(prod["spec"]["voterCount"], serde_yaml::Value::from(3));
    assert_eq!(prod["spec"]["auth"], "required");

    // --name/--namespace/--image overrides flow into the CR.
    let custom: serde_yaml::Value = serde_yaml::from_str(&stdout(&[
        "k8s",
        "instance",
        "render",
        "--profile",
        "dev",
        "--name",
        "relay",
        "--namespace",
        "queues",
        "--image",
        "relay:pinned",
    ]))
    .unwrap();
    assert_eq!(custom["metadata"]["namespace"], "queues");
    assert_eq!(custom["spec"]["image"], "relay:pinned");

    // Dockerfile render succeeds for both variants (content asserted below).
    stdout(&["dockerfile", "render", "--variant", "source"]);
    stdout(&["dockerfile", "render", "--variant", "release"]);

    // operator run without the feature: nonzero exit + rebuild hint. (In an
    // `--features operator` build the verb runs the real controller — never
    // invoke it from a test, it would watch whatever cluster kubeconfig names.)
    #[cfg(not(feature = "operator"))]
    {
        let out = run(&["k8s", "operator", "run"]);
        assert!(
            !out.status.success(),
            "operator run must fail without --features operator"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("--features operator"),
            "rebuild hint names the feature"
        );
    }
}

/// R4 / AC2: `--variant source` reproduces the committed `Dockerfile`
/// byte-for-byte, `--variant release` the committed `Dockerfile.release`, and
/// an explicit `--version` flows into the ARG + tag lines.
#[test]
fn dockerfile_render_reproduces_committed_fixtures() {
    let rendered = stdout(&["dockerfile", "render", "--variant", "source"]);
    assert_eq!(
        rendered,
        include_str!("../../../../Dockerfile"),
        "relay dockerfile render --variant source == committed Dockerfile"
    );

    let rendered = stdout(&["dockerfile", "render", "--variant", "release"]);
    assert_eq!(
        rendered,
        include_str!("../../../../Dockerfile.release"),
        "relay dockerfile render --variant release == committed Dockerfile.release"
    );

    let pinned = stdout(&[
        "dockerfile",
        "render",
        "--variant",
        "release",
        "--version",
        "9.9.9",
    ]);
    assert!(
        pinned.contains("ARG RELAY_VERSION=relay@9.9.9"),
        "pinned ARG: {pinned}"
    );
    assert!(
        pinned.contains("-t relay:9.9.9"),
        "pinned image tag: {pinned}"
    );
}

/// R3 / AC4: the rendered CRD (the committed fixture in this default build) is
/// Kubernetes structural-schema safe and declares the Relay kind.
#[test]
fn crd_render_is_structural_schema_safe() {
    let yaml = stdout(&["k8s", "crd", "render"]);
    assert!(
        !yaml.contains("uint32"),
        "CRD must not carry format: uint32"
    );
    assert!(
        !yaml.contains("uint64"),
        "CRD must not carry format: uint64"
    );
    assert!(
        yaml.contains("minimum"),
        "normalized uints keep a minimum floor"
    );

    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("CRD parses as YAML");
    assert_eq!(doc["kind"], "CustomResourceDefinition");
    assert_eq!(doc["spec"]["group"], "relay.dev");
    assert_eq!(doc["spec"]["names"]["kind"], "Relay");
    assert_eq!(doc["metadata"]["name"], "relays.relay.dev");
    assert!(
        yaml.contains("default: \"off\""),
        "Kubernetes YAML 1.1 must preserve the auth default as a string"
    );
}

/// R6: the GKE test overlay (k8s/overlays/gke-test, the cluster of the
/// `raft_failover` e2e suite) runs the single `relay` auto-mode image as three
/// voters: the base StatefulSet carries the standard downward-API env and the
/// peer service on 7000, and the overlay raises it to three replicas and
/// voters, leaving the image under test for the workflow to pin.
#[test]
fn gke_test_overlay_is_three_voter_auto_mode() {
    let base = parse_yaml_docs(
        "base statefulset",
        include_str!("../../../../k8s/base/statefulset.yaml"),
    );
    let config = parse_yaml_docs(
        "base configmap",
        include_str!("../../../../k8s/base/configmap.yaml"),
    );
    let patch = parse_yaml_docs(
        "gke-test statefulset patch",
        include_str!("../../../../k8s/overlays/gke-test/statefulset-patch.yaml"),
    );
    let overlay = parse_yaml_docs(
        "gke-test kustomization",
        include_str!("../../../../k8s/overlays/gke-test/kustomization.yaml"),
    );
    let env = |sts: &serde_yaml::Value, k: &str| {
        sts["spec"]["template"]["spec"]["containers"][0]["env"]
            .as_sequence()
            .expect("env sequence")
            .iter()
            .find(|e| e["name"] == k)
            .cloned()
            .unwrap_or_else(|| panic!("StatefulSet missing env {k}"))
    };

    // The auto-mode contract from the base: the downward-API pod name, one
    // shard, the peer service, and the serve port 7000.
    let base = &base[0];
    assert_eq!(env(base, "SHARD_COUNT")["value"], "1");
    assert_eq!(
        env(base, "POD_NAME")["valueFrom"]["fieldRef"]["fieldPath"],
        "metadata.name"
    );
    assert_eq!(env(base, "RELAY_PEER_SERVICE")["value"], "relay-headless");
    assert_eq!(
        env(base, "RELAY_BIND")["valueFrom"]["configMapKeyRef"]["key"],
        "RELAY_BIND"
    );
    assert_eq!(config[0]["data"]["RELAY_BIND"], "0.0.0.0:7000");

    // The overlay: three replicas and three voters of the one `relay` image.
    let patch = &patch[0];
    assert_eq!(patch["spec"]["replicas"], serde_yaml::Value::from(3));
    assert_eq!(env(patch, "REPLICAS_PER_SHARD")["value"], "3");
    assert_eq!(env(patch, "VOTER_COUNT")["value"], "3");
    assert_eq!(
        patch["spec"]["template"]["spec"]["containers"][0]["image"],
        "ghcr.io/faberline/relay"
    );
    let overlay = &overlay[0];
    assert_eq!(overlay["resources"][0], "../../base");
    assert_eq!(overlay["images"][0]["name"], "ghcr.io/faberline/relay");
    assert_eq!(overlay["images"][0]["newTag"], "REPLACE_ME");
}

/// R7: the llm operations topic documents the deploy verbs so the outline
/// stays honest about the CLI surface.
#[test]
fn llm_operations_topic_names_deploy_verbs() {
    let topic = stdout(&["llm", "operations"]);
    for needle in [
        "k8s crd render",
        "k8s operator render",
        "k8s operator run",
        "k8s instance render",
        "dockerfile render",
    ] {
        assert!(
            topic.contains(needle),
            "operations topic missing `{needle}`"
        );
    }
}

#[test]
fn file_render_ends_with_explicit_terminal_marker() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("relay.yaml");
    let output = stdout(&[
        "k8s",
        "instance",
        "render",
        "--profile",
        "dev",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(out.is_file());
    assert!(output.lines().any(|line| line == "next: done"));
}
// HANDWRITE-END
