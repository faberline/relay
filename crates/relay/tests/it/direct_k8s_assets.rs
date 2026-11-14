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

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("{} is missing or unreadable: {err}", path.display()))
}

fn kinds_of_file(path: &Path) -> Vec<String> {
    let text = read(path);
    serde_yaml::Deserializer::from_str(&text)
        .map(|doc| Value::deserialize(doc).expect("document parses"))
        .filter_map(|doc| doc["kind"].as_str().map(str::to_owned))
        .collect()
}

/// Kinds of every object that `dir/kustomization.yaml` reaches through
/// `resources` and `components`, recursing into directories. Every entry is
/// resolved against the directory of the kustomization that names it.
fn compose(dir: &Path, seen: &mut Vec<String>) -> Vec<String> {
    let kustomization = yaml(&read(&dir.join("kustomization.yaml")));
    let mut kinds = Vec::new();
    for key in ["resources", "components"] {
        let Some(entries) = kustomization[key].as_sequence() else {
            continue;
        };
        for entry in entries {
            let rel = entry
                .as_str()
                .unwrap_or_else(|| panic!("{key} entry in {} is not a path", dir.display()));
            let path = dir.join(rel);
            seen.push(path.display().to_string());
            if path.is_dir() {
                kinds.extend(compose(&path, seen));
            } else {
                assert!(path.is_file(), "{} does not exist", path.display());
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
        let mut seen = Vec::new();
        let kinds = compose(&dir, &mut seen);
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
                "overlay {overlay} does not compose a {kind}"
            );
        }
        let with_components = matches!(overlay, "staging" | "prod");
        for kind in ["NetworkPolicy", "ServiceMonitor", "PrometheusRule"] {
            let present = kinds.iter().any(|k| k == kind);
            assert_eq!(
                present, with_components,
                "overlay {overlay}: {kind} present = {present}, expected {with_components}"
            );
        }
        let kustomization = yaml(&read(&dir.join("kustomization.yaml")));
        if let Some(patches) = kustomization["patches"].as_sequence() {
            for patch in patches {
                if let Some(path) = patch["path"].as_str() {
                    assert!(
                        dir.join(path).is_file(),
                        "overlay {overlay} patches with missing file {path}"
                    );
                }
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 5, "the five overlays must all be checked");
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
        let entries = std::fs::read_dir(&path)
            .unwrap_or_else(|err| panic!("{} is unreadable: {err}", path.display()));
        for entry in entries {
            let file = entry.expect("directory entry").path();
            if file.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }
            let text = read(&file);
            assert!(
                !text.contains("REPLACE_ME"),
                "{} has an unresolved REPLACE_ME",
                file.display()
            );
            assert!(
                !text.contains("${"),
                "{} has an unresolved ${{...}} placeholder",
                file.display()
            );
        }
    }
    // Negative: the two template overlays keep their marker. Resolving it
    // without updating the workflow contract must fail this test.
    for dir in ["overlays/gke-test", "overlays/template"] {
        let text = read(&k8s_root().join(dir).join("kustomization.yaml"));
        assert!(
            text.contains("REPLACE_ME"),
            "{dir} must keep its REPLACE_ME marker for the workflow"
        );
    }
}

#[test]
fn dockerfiles_run_non_root_with_the_relay_entrypoint() {
    let dockerfile = include_str!("../../../../Dockerfile");
    let release = include_str!("../../../../Dockerfile.release");
    let entrypoint = r#"ENTRYPOINT ["/usr/local/bin/relay"]"#;
    for (name, text) in [("Dockerfile", dockerfile), ("Dockerfile.release", release)] {
        assert!(
            text.lines().any(|line| line.trim() == entrypoint),
            "{name} lacks `{entrypoint}`"
        );
    }
    assert!(
        dockerfile.lines().any(|line| line.trim() == "USER relay"),
        "Dockerfile does not run as USER relay"
    );
    assert!(
        release.contains("gcr.io/distroless/static-debian12:nonroot"),
        "Dockerfile.release is not on the nonroot distroless base"
    );
    let statefulset = yaml(include_str!("../../../../k8s/base/statefulset.yaml"));
    let container = &statefulset["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(
        container["command"][0], "relay",
        "container command must be relay"
    );
    assert!(
        container["image"]
            .as_str()
            .is_some_and(|image| image.starts_with("relay:")),
        "base image must be relay:<tag>"
    );
    let gke = include_str!("../../../../k8s/overlays/gke-test/kustomization.yaml");
    assert!(
        gke.contains("ghcr.io/faberline/relay"),
        "gke-test must pin the image name ghcr.io/faberline/relay"
    );
}

/// Every YAML document of one file, in order.
fn docs_of(name: &str, text: &str) -> Vec<Value> {
    serde_yaml::Deserializer::from_str(text)
        .map(|doc| {
            Value::deserialize(doc).unwrap_or_else(|err| panic!("{name} does not parse: {err}"))
        })
        .collect()
}

/// The StatefulSet, headless Service, client Service and PDB form one raft
/// peer topology: a drift in any link breaks peer discovery.
#[test]
fn statefulset_raft_service_topology_links_in_base_assets() {
    let sts = yaml(include_str!("../../../../k8s/base/statefulset.yaml"));
    let services = docs_of(
        "k8s/base/service.yaml",
        include_str!("../../../../k8s/base/service.yaml"),
    );
    let pdb = yaml(include_str!("../../../../k8s/base/pdb.yaml"));
    assert_eq!(
        services.len(),
        2,
        "k8s/base/service.yaml must hold the headless and the client Service"
    );
    let headless = services
        .iter()
        .find(|s| s["spec"]["clusterIP"] == "None")
        .expect("k8s/base/service.yaml has no Service with clusterIP None");
    let client = services
        .iter()
        .find(|s| s["spec"]["clusterIP"].is_null())
        .expect("k8s/base/service.yaml has no client Service");
    let headless_name = headless["metadata"]["name"]
        .as_str()
        .expect("headless Service has a name");

    assert_eq!(
        sts["spec"]["serviceName"], headless_name,
        "StatefulSet spec.serviceName must name the headless Service"
    );
    assert_eq!(
        headless["spec"]["publishNotReadyAddresses"], true,
        "headless Service must publishNotReadyAddresses so voters find each other before ready"
    );

    let container = &sts["spec"]["template"]["spec"]["containers"][0];
    let container_ports = container["ports"].as_sequence().expect("container ports");
    let container_port = |name: &str| -> u64 {
        container_ports
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("StatefulSet container has no port named {name}"))
            ["containerPort"]
            .as_u64()
            .expect("containerPort is a number")
    };
    let service_ports = headless["spec"]["ports"]
        .as_sequence()
        .expect("headless ports");
    for (name, number) in [("http", 7000u64), ("raft", 7001u64)] {
        let port = service_ports
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("headless Service has no port named {name}"));
        assert_eq!(port["port"], number, "headless Service port {name} number");
        assert_eq!(
            port["targetPort"], name,
            "headless Service port {name} must target the container port name"
        );
        assert_eq!(
            container_port(name),
            number,
            "StatefulSet container port {name} must equal the Service port"
        );
    }

    let env = container["env"].as_sequence().expect("container env");
    let env_value = |key: &str| -> Value {
        env.iter()
            .find(|e| e["name"] == key)
            .unwrap_or_else(|| panic!("StatefulSet container has no env {key}"))["value"]
            .clone()
    };
    assert_eq!(
        env_value("RELAY_PEER_SERVICE"),
        headless["metadata"]["name"],
        "env RELAY_PEER_SERVICE must equal the headless Service name"
    );
    assert_eq!(
        env_value("RELAY_RAFT_PORT"),
        Value::String(container_port("raft").to_string()),
        "env RELAY_RAFT_PORT must equal the raft container port"
    );

    let pod_labels = &sts["spec"]["template"]["metadata"]["labels"];
    assert!(pod_labels.is_mapping(), "pod template labels must be a map");
    assert_eq!(
        &sts["spec"]["selector"]["matchLabels"], pod_labels,
        "StatefulSet selector.matchLabels must equal the pod template labels"
    );
    assert_eq!(
        &headless["spec"]["selector"], pod_labels,
        "headless Service selector must equal the pod labels"
    );
    assert_eq!(
        &client["spec"]["selector"], pod_labels,
        "client Service selector must equal the pod labels"
    );
    assert_eq!(
        &pdb["spec"]["selector"]["matchLabels"], pod_labels,
        "PodDisruptionBudget selector.matchLabels must equal the pod labels"
    );
}

/// The gke-test overlay keeps three voters, one per node.
#[test]
fn gke_test_overlay_merged_intent_and_affinity() {
    let base = yaml(include_str!("../../../../k8s/base/statefulset.yaml"));
    let patch = yaml(include_str!(
        "../../../../k8s/overlays/gke-test/statefulset-patch.yaml"
    ));
    assert_eq!(
        base["spec"]["podManagementPolicy"], "Parallel",
        "base StatefulSet podManagementPolicy must stay Parallel"
    );
    assert_eq!(
        patch["spec"]["replicas"], 3,
        "gke-test patch must set replicas 3"
    );
    let env = patch["spec"]["template"]["spec"]["containers"][0]["env"]
        .as_sequence()
        .expect("gke-test patch container env");
    for key in ["REPLICAS_PER_SHARD", "VOTER_COUNT"] {
        let entry = env
            .iter()
            .find(|e| e["name"] == key)
            .unwrap_or_else(|| panic!("gke-test patch has no env {key}"));
        assert_eq!(
            entry["value"], "3",
            "gke-test patch env {key} must equal replicas 3"
        );
    }
    let terms = patch["spec"]["template"]["spec"]["affinity"]["podAntiAffinity"]
        ["requiredDuringSchedulingIgnoredDuringExecution"]
        .as_sequence()
        .expect("gke-test patch needs a required pod anti-affinity");
    let term = terms
        .iter()
        .find(|t| t["topologyKey"] == "kubernetes.io/hostname")
        .expect("gke-test anti-affinity must use topologyKey kubernetes.io/hostname");
    assert_eq!(
        term["labelSelector"]["matchLabels"], base["spec"]["template"]["metadata"]["labels"],
        "gke-test anti-affinity matchLabels must equal the base pod labels"
    );
}
// HANDWRITE-END
