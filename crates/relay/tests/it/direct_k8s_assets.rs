// HANDWRITE-BEGIN gap="missing-generator:unit-test:relay-direct-k8s" tracker="#1208" reason="Offline contract for Relay direct Kustomize singleton and operator-owned HA boundary."
use serde::Deserialize;
use serde_yaml::Value;
use std::path::{Path, PathBuf};

fn yaml(input: &str) -> Value {
    serde_yaml::from_str(input).expect("manifest parses")
}

#[test]
fn direct_base_is_a_restricted_durable_singleton() {
    let statefulset = yaml(include_str!("../../../../k8s/base/statefulset.yaml"));
    assert_eq!(statefulset["kind"], "StatefulSet");
    assert_eq!(statefulset["spec"]["replicas"], 1);
    assert_eq!(statefulset["spec"]["serviceName"], "relay-headless");
    assert_eq!(
        statefulset["spec"]["volumeClaimTemplates"][0]["spec"]["resources"]["requests"]["storage"],
        "10Gi"
    );
    let container = &statefulset["spec"]["template"]["spec"]["containers"][0];
    let pod_security = &statefulset["spec"]["template"]["spec"]["securityContext"];
    assert_eq!(pod_security["runAsNonRoot"], true);
    assert_eq!(pod_security["runAsUser"], 65532);
    assert_eq!(pod_security["seccompProfile"]["type"], "RuntimeDefault");
    assert_eq!(container["command"][0], "relay");
    assert_eq!(container["ports"][0]["containerPort"], 7000);
    assert_eq!(container["ports"][1]["containerPort"], 7001);
    assert_eq!(container["securityContext"]["readOnlyRootFilesystem"], true);
    assert_eq!(
        container["securityContext"]["allowPrivilegeEscalation"],
        false
    );
    assert_eq!(
        container["securityContext"]["capabilities"]["drop"][0],
        "ALL"
    );
    let env = container["env"].as_sequence().unwrap();
    assert!(env.iter().any(|entry| entry["name"] == "RELAY_LOG_FORMAT"));
    let config = yaml(include_str!("../../../../k8s/base/configmap.yaml"));
    assert_eq!(config["data"]["RELAY_LOG_FORMAT"], "json");
    for (name, value) in [
        ("SHARD_COUNT", "1"),
        ("REPLICAS_PER_SHARD", "1"),
        ("VOTER_COUNT", "1"),
        ("RELAY_PEER_SERVICE", "relay-headless"),
        ("RELAY_RAFT_PORT", "7001"),
        ("RELAY_DATA_DIR", "/data"),
    ] {
        assert!(env
            .iter()
            .any(|entry| entry["name"] == name && entry["value"] == value));
    }
}

#[test]
fn prod_profile_uses_security_components_without_voter_hpa() {
    let prod = include_str!("../../../../k8s/overlays/prod/kustomization.yaml");
    assert!(prod.contains("RELAY_TOKEN_REGISTRY_FILE"));
    assert!(prod.contains("secretName: relay-token-registry"));
    assert!(prod.contains("readOnly: true"));
    assert!(prod.contains("../../components/observability"));
    assert!(prod.contains("../../components/network-policy"));
    assert!(!prod.contains("HorizontalPodAutoscaler"));
    assert!(!prod.contains("kind: HPA"));

    let policy = yaml(include_str!(
        "../../../../k8s/components/network-policy/networkpolicy.yaml"
    ));
    assert_eq!(policy["kind"], "NetworkPolicy");
    assert_eq!(policy["spec"]["podSelector"]["matchLabels"]["app"], "relay");
    let policy_types = policy["spec"]["policyTypes"].as_sequence().unwrap();
    assert!(policy_types.iter().any(|value| value == "Ingress"));
    assert!(policy_types.iter().any(|value| value == "Egress"));
    let policy_yaml = serde_yaml::to_string(&policy).unwrap();
    assert!(policy_yaml.contains("port: 7000"));
    assert!(policy_yaml.contains("port: 7001"));
}

fn k8s_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../k8s")
}

fn kinds_of_file(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("{}: cannot read: {err}", path.display()));
    serde_yaml::Deserializer::from_str(&text)
        .filter_map(|document| {
            let value = Value::deserialize(document)
                .unwrap_or_else(|err| panic!("{}: document does not parse: {err}", path.display()));
            value["kind"].as_str().map(str::to_owned)
        })
        .collect()
}

// Kinds of every object that a kustomization reaches through `resources` and
// `components`. Relative entries resolve against the kustomization's directory.
fn compose(dir: &Path) -> Vec<String> {
    let kustomization = dir.join("kustomization.yaml");
    let text = std::fs::read_to_string(&kustomization)
        .unwrap_or_else(|err| panic!("{}: cannot read: {err}", kustomization.display()));
    let doc = yaml(&text);
    let mut kinds = Vec::new();
    for key in ["resources", "components"] {
        let Some(entries) = doc[key].as_sequence() else {
            continue;
        };
        for entry in entries {
            let rel = entry.as_str().unwrap_or_else(|| {
                panic!("{}: {key} entry is not a string", kustomization.display())
            });
            let path = dir.join(rel);
            if path.is_dir() {
                kinds.extend(compose(&path));
            } else {
                assert!(
                    path.is_file(),
                    "{}: lists {} but the file is missing",
                    kustomization.display(),
                    path.display()
                );
                kinds.extend(kinds_of_file(&path));
            }
        }
    }
    kinds
}

#[test]
fn every_overlay_composes_from_existing_files() {
    let overlays = ["dev", "staging", "prod", "gke-test", "template"];
    let mut checked = 0;
    for overlay in overlays {
        let dir = k8s_root().join("overlays").join(overlay);
        let kinds = compose(&dir);
        for kind in [
            "Namespace",
            "ServiceAccount",
            "ConfigMap",
            "StatefulSet",
            "Service",
            "PodDisruptionBudget",
        ] {
            assert!(
                kinds.iter().any(|k| k == kind),
                "overlay {overlay}: composed output has no {kind}"
            );
        }
        let has_observability = matches!(overlay, "staging" | "prod");
        for kind in ["NetworkPolicy", "ServiceMonitor", "PrometheusRule"] {
            assert_eq!(
                kinds.iter().any(|k| k == kind),
                has_observability,
                "overlay {overlay}: presence of {kind} must be {has_observability}"
            );
        }
        let kustomization = yaml(
            &std::fs::read_to_string(dir.join("kustomization.yaml"))
                .unwrap_or_else(|err| panic!("overlay {overlay}: {err}")),
        );
        if let Some(patches) = kustomization["patches"].as_sequence() {
            for patch in patches {
                if let Some(path) = patch["path"].as_str() {
                    assert!(
                        dir.join(path).is_file(),
                        "overlay {overlay}: patch file {path} is missing"
                    );
                }
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 5, "all five overlays are checked");
}

#[test]
fn shipped_overlays_have_no_unresolved_placeholder() {
    for dir in [
        "base",
        "components/network-policy",
        "components/observability",
        "overlays/dev",
        "overlays/staging",
        "overlays/prod",
    ] {
        let path = k8s_root().join(dir);
        let mut files = 0;
        for entry in std::fs::read_dir(&path)
            .unwrap_or_else(|err| panic!("{}: cannot read dir: {err}", path.display()))
        {
            let file = entry.expect("directory entry").path();
            if file.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|err| panic!("{}: cannot read: {err}", file.display()));
            for marker in ["REPLACE_ME", "${"] {
                assert!(
                    !text.contains(marker),
                    "{}: unresolved placeholder {marker}",
                    file.display()
                );
            }
            files += 1;
        }
        assert!(files > 0, "{}: no yaml files found", path.display());
    }
    // The two template overlays keep their marker: the workflow replaces it.
    for dir in ["overlays/gke-test", "overlays/template"] {
        let text = std::fs::read_to_string(k8s_root().join(dir).join("kustomization.yaml"))
            .unwrap_or_else(|err| panic!("{dir}: cannot read: {err}"));
        assert!(
            text.contains("REPLACE_ME"),
            "{dir}: the REPLACE_ME marker must stay for the workflow"
        );
    }
}

#[test]
fn dockerfiles_run_non_root_with_the_relay_entrypoint() {
    let source = include_str!("../../../../Dockerfile");
    let release = include_str!("../../../../Dockerfile.release");
    let entrypoint = r#"ENTRYPOINT ["/usr/local/bin/relay"]"#;
    assert!(
        source.lines().any(|line| line.trim() == entrypoint),
        "Dockerfile: missing {entrypoint}"
    );
    assert!(
        release.lines().any(|line| line.trim() == entrypoint),
        "Dockerfile.release: missing {entrypoint}"
    );
    assert!(
        source.lines().any(|line| line.trim() == "USER relay"),
        "Dockerfile: missing USER relay"
    );
    assert!(
        release.contains("gcr.io/distroless/static-debian12:nonroot"),
        "Dockerfile.release: missing the nonroot distroless runtime"
    );
    let statefulset = yaml(include_str!("../../../../k8s/base/statefulset.yaml"));
    let container = &statefulset["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(container["command"][0], "relay");
    assert!(
        container["image"]
            .as_str()
            .is_some_and(|image| image.starts_with("relay:")),
        "base image must be the local relay:<version> name"
    );
    let gke = include_str!("../../../../k8s/overlays/gke-test/kustomization.yaml");
    assert!(
        gke.contains("ghcr.io/faberline/relay"),
        "gke-test overlay must set the image under test"
    );
}
// HANDWRITE-END
