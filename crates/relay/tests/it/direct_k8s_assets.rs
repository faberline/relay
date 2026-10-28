// HANDWRITE-BEGIN gap="missing-generator:unit-test:relay-direct-k8s" tracker="#1208" reason="Offline contract for Relay direct Kustomize singleton and operator-owned HA boundary."
use serde_yaml::Value;

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

fn yaml_docs(input: &str) -> Vec<Value> {
    use serde::Deserialize;
    serde_yaml::Deserializer::from_str(input)
        .filter_map(|doc| {
            let v = Value::deserialize(doc).expect("YAML document parses");
            if v.is_null() {
                None
            } else {
                Some(v)
            }
        })
        .collect()
}

#[test]
fn statefulset_raft_service_topology_links_in_base_assets() {
    let statefulset = yaml(include_str!("../../../../k8s/base/statefulset.yaml"));
    let services = yaml_docs(include_str!("../../../../k8s/base/service.yaml"));
    let pdb = yaml(include_str!("../../../../k8s/base/pdb.yaml"));

    let headless = services
        .iter()
        .find(|s| s["spec"]["clusterIP"] == "None")
        .expect("k8s/base/service.yaml must contain a headless Service with clusterIP: None");
    let client = services
        .iter()
        .find(|s| s["spec"]["clusterIP"] != "None")
        .expect("k8s/base/service.yaml must contain a client Service");

    // StatefulSet spec.serviceName equals the name of the clusterIP: None Service
    assert_eq!(
        statefulset["spec"]["serviceName"], headless["metadata"]["name"],
        "StatefulSet spec.serviceName must match headless Service metadata.name"
    );

    // That Service has publishNotReadyAddresses: true
    assert_eq!(
        headless["spec"]["publishNotReadyAddresses"], true,
        "Headless Service spec.publishNotReadyAddresses must be true"
    );

    // StatefulSet container ports
    let container = &statefulset["spec"]["template"]["spec"]["containers"][0];
    let container_ports = container["ports"]
        .as_sequence()
        .expect("StatefulSet container must have ports sequence");
    let http_container_port = container_ports
        .iter()
        .find(|p| p["containerPort"] == 7000)
        .expect("StatefulSet container must have port 7000");
    let raft_container_port = container_ports
        .iter()
        .find(|p| p["containerPort"] == 7001)
        .expect("StatefulSet container must have port 7001");

    assert_eq!(
        http_container_port["name"], "http",
        "StatefulSet container port 7000 name must be http"
    );
    assert_eq!(
        raft_container_port["name"], "raft",
        "StatefulSet container port 7001 name must be raft"
    );

    // Headless Service ports http 7000 and raft 7001 targeting container port names
    let headless_ports = headless["spec"]["ports"]
        .as_sequence()
        .expect("Headless Service must have ports sequence");
    let headless_http = headless_ports
        .iter()
        .find(|p| p["port"] == 7000)
        .expect("Headless Service must expose port 7000");
    let headless_raft = headless_ports
        .iter()
        .find(|p| p["port"] == 7001)
        .expect("Headless Service must expose port 7001");

    assert_eq!(
        headless_http["targetPort"], http_container_port["name"],
        "Headless Service http targetPort must match StatefulSet container port name"
    );
    assert_eq!(
        headless_raft["targetPort"], raft_container_port["name"],
        "Headless Service raft targetPort must match StatefulSet container port name"
    );

    // Container env RELAY_PEER_SERVICE equals Service name and RELAY_RAFT_PORT equals raft container port
    let env = container["env"]
        .as_sequence()
        .expect("StatefulSet container env sequence");
    let peer_service_env = env
        .iter()
        .find(|e| e["name"] == "RELAY_PEER_SERVICE")
        .expect("StatefulSet container must have RELAY_PEER_SERVICE env var");
    assert_eq!(
        peer_service_env["value"], headless["metadata"]["name"],
        "Container env RELAY_PEER_SERVICE must match headless Service metadata.name"
    );

    let raft_port_env = env
        .iter()
        .find(|e| e["name"] == "RELAY_RAFT_PORT")
        .expect("StatefulSet container must have RELAY_RAFT_PORT env var");
    assert_eq!(
        raft_port_env["value"],
        raft_container_port["containerPort"]
            .as_u64()
            .unwrap()
            .to_string(),
        "Container env RELAY_RAFT_PORT must match container raft port"
    );

    // spec.selector.matchLabels, pod template labels, both Services' selectors, and pdb.yaml selector
    // are all the same label map
    let pod_labels = &statefulset["spec"]["template"]["metadata"]["labels"];
    assert_eq!(
        statefulset["spec"]["selector"]["matchLabels"], *pod_labels,
        "StatefulSet selector.matchLabels must match pod template labels"
    );
    assert_eq!(
        headless["spec"]["selector"], *pod_labels,
        "Headless Service selector must match pod template labels"
    );
    assert_eq!(
        client["spec"]["selector"], *pod_labels,
        "Client Service selector must match pod template labels"
    );
    assert_eq!(
        pdb["spec"]["selector"]["matchLabels"], *pod_labels,
        "PDB selector.matchLabels must match pod template labels"
    );
}

#[test]
fn gke_test_overlay_merged_intent_and_affinity() {
    let base_sts = yaml(include_str!("../../../../k8s/base/statefulset.yaml"));
    let patch = yaml(include_str!(
        "../../../../k8s/overlays/gke-test/statefulset-patch.yaml"
    ));

    // replicas 3 equals REPLICAS_PER_SHARD and VOTER_COUNT
    assert_eq!(
        patch["spec"]["replicas"], 3,
        "StatefulSet patch spec.replicas must be 3"
    );
    let patch_env = patch["spec"]["template"]["spec"]["containers"][0]["env"]
        .as_sequence()
        .expect("StatefulSet patch container env sequence");
    let replicas_per_shard = patch_env
        .iter()
        .find(|e| e["name"] == "REPLICAS_PER_SHARD")
        .expect("StatefulSet patch must set REPLICAS_PER_SHARD");
    assert_eq!(
        replicas_per_shard["value"],
        patch["spec"]["replicas"].as_u64().unwrap().to_string(),
        "Container env REPLICAS_PER_SHARD must equal StatefulSet patch replicas"
    );
    let voter_count = patch_env
        .iter()
        .find(|e| e["name"] == "VOTER_COUNT")
        .expect("StatefulSet patch must set VOTER_COUNT");
    assert_eq!(
        voter_count["value"],
        patch["spec"]["replicas"].as_u64().unwrap().to_string(),
        "Container env VOTER_COUNT must equal StatefulSet patch replicas"
    );

    // podManagementPolicy stays Parallel in the base
    assert_eq!(
        base_sts["spec"]["podManagementPolicy"], "Parallel",
        "Base StatefulSet podManagementPolicy must be Parallel"
    );

    // Patch has requiredDuringSchedulingIgnoredDuringExecution pod anti-affinity on kubernetes.io/hostname
    // whose matchLabels equal base pod labels
    let anti_affinity_rules = patch["spec"]["template"]["spec"]["affinity"]["podAntiAffinity"]
        ["requiredDuringSchedulingIgnoredDuringExecution"]
        .as_sequence()
        .expect("StatefulSet patch must have requiredDuringSchedulingIgnoredDuringExecution rules");
    let hostname_rule = anti_affinity_rules
        .iter()
        .find(|r| r["topologyKey"] == "kubernetes.io/hostname")
        .expect("podAntiAffinity must contain rule with topologyKey kubernetes.io/hostname");
    let base_pod_labels = &base_sts["spec"]["template"]["metadata"]["labels"];
    assert_eq!(
        hostname_rule["labelSelector"]["matchLabels"], *base_pod_labels,
        "podAntiAffinity matchLabels must equal base pod template labels"
    );
}
// HANDWRITE-END
