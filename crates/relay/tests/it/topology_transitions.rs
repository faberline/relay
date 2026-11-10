// HANDWRITE-BEGIN gap="missing-generator:unit-test:topology-transitions" tracker="#46" reason="Render/status assertions for direct CR topology targets; no live cluster."
//! Direct topology transitions of the `Relay` CR (issue #46, narrowed scope).
//!
//! The journey `1x1 -> 2x1 -> 3x1 -> 3x2 -> 3x3 -> 3x2 -> 3x1 -> 2x1 -> 1x1`
//! (`shardCount x replicasPerShard`) is expressed only through the CR spec.
//! These tests check the pure render and status output for each target. A
//! live cluster, shard split/merge and dynamic Raft membership are out of scope.
#![cfg(feature = "operator")]

use std::collections::{BTreeSet, HashMap};

use relay::operator::render::render;
use relay::operator::{Relay, RelaySpec};
use serde_json::Value;
use service_k8s::{ClusterSpec, ManagedService, ReadyFacts};

/// `(shardCount, replicasPerShard)` for each step of the journey.
const JOURNEY: [(u32, u32); 9] = [
    (1, 1),
    (2, 1),
    (3, 1),
    (3, 2),
    (3, 3),
    (3, 2),
    (3, 1),
    (2, 1),
    (1, 1),
];

fn spec(shards: u32, replicas: u32) -> RelaySpec {
    RelaySpec {
        cluster: ClusterSpec {
            image: "relay:test".into(),
            image_pull_policy: None,
            shard_count: shards,
            replicas_per_shard: replicas,
            voter_count: replicas,
            resources: Default::default(),
        },
        storage: "10Gi".into(),
        storage_class: None,
        grace_secs: 10,
        log_level: None,
        auth: "off".into(),
        tokens_secret: None,
        peer_tls_secret: None,
        body_limit_bytes: 8 * 1024 * 1024,
        backup: None,
    }
}

fn of_kind<'a>(objs: &'a [Value], kind: &str) -> &'a Value {
    objs.iter()
        .find(|o| o["kind"] == kind)
        .unwrap_or_else(|| panic!("render output has no {kind}"))
}

fn env_of(sts: &Value) -> Vec<(&str, &Value)> {
    sts["spec"]["template"]["spec"]["containers"][0]["env"]
        .as_array()
        .expect("container env array")
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e))
        .collect()
}

/// The `value` of the StatefulSet env entry named `key`.
fn env_value(sts: &Value, key: &str) -> String {
    env_of(sts)
        .into_iter()
        .find(|(name, _)| *name == key)
        .map(|(_, e)| {
            e["value"]
                .as_str()
                .expect("env value is a string")
                .to_string()
        })
        .unwrap_or_else(|| panic!("StatefulSet env has no {key}"))
}

#[test]
fn replica_targets_render_whole_layers() {
    for (_, replicas) in JOURNEY {
        let objs = render(&Relay::new("relay", spec(1, replicas)));
        let sts = of_kind(&objs, "StatefulSet");
        assert_eq!(
            sts["spec"]["replicas"], replicas,
            "replicas for target {replicas}"
        );
        assert_eq!(env_value(sts, "REPLICAS_PER_SHARD"), replicas.to_string());
        assert_eq!(env_value(sts, "VOTER_COUNT"), replicas.to_string());
    }
}

// Negative: fails if render ever starts honoring `shardCount`. Relay is one raft
// group today; real shard split/merge is a separate Relay-domain issue.
#[test]
fn shard_count_target_is_pinned_to_one() {
    for shards in [1, 2, 3] {
        let objs = render(&Relay::new("relay", spec(shards, 3)));
        let sts = of_kind(&objs, "StatefulSet");
        assert_eq!(
            sts["spec"]["replicas"], 3,
            "shardCount {shards} must not change replicas"
        );
        assert_eq!(
            env_value(sts, "SHARD_COUNT"),
            "1",
            "shardCount {shards} must render SHARD_COUNT 1"
        );
    }
}

#[test]
fn journey_renders_no_autoscaler_and_only_cr_driven_objects() {
    let expected: BTreeSet<&str> = [
        "ServiceAccount",
        "StatefulSet",
        "Service",
        "PodDisruptionBudget",
    ]
    .into_iter()
    .collect();
    for (shards, replicas) in JOURNEY {
        let relay = Relay::new("relay", spec(shards, replicas));
        let objs = render(&relay);

        for obj in &objs {
            assert_ne!(
                obj["kind"], "HorizontalPodAutoscaler",
                "no HPA may be rendered"
            );
            assert!(
                obj["metadata"]["annotations"]["autoscaling"].is_null(),
                "no object may carry an autoscaling annotation"
            );
        }
        let kinds: BTreeSet<&str> = objs.iter().filter_map(|o| o["kind"].as_str()).collect();
        assert_eq!(kinds, expected, "kinds for target {shards}x{replicas}");

        assert_eq!(
            render(&relay),
            objs,
            "rendering the same CR twice must be deterministic"
        );

        // Resource size never requests a topology change.
        let mut resized = spec(shards, replicas);
        resized.cluster.resources.cpu = "8".into();
        resized.cluster.resources.memory = "32Gi".into();
        let sized = render(&Relay::new("relay", resized));
        let before = of_kind(&objs, "StatefulSet");
        let after = of_kind(&sized, "StatefulSet");
        assert_eq!(after["spec"]["replicas"], before["spec"]["replicas"]);
        for key in ["REPLICAS_PER_SHARD", "VOTER_COUNT", "SHARD_COUNT"] {
            assert_eq!(
                env_value(after, key),
                env_value(before, key),
                "{key} must not change"
            );
        }
    }
}

#[test]
fn status_patch_follows_each_transition() {
    let replicas: Vec<u32> = JOURNEY.iter().map(|(_, r)| *r).collect();
    for (i, pair) in replicas.windows(2).enumerate() {
        let (prev, next) = (pair[0], pair[1]);
        let generation = i as i64 + 2;
        let mut relay = Relay::new("relay", spec(1, next));
        relay.metadata.generation = Some(generation);
        let ready = ReadyFacts {
            ready: HashMap::from([("relay".to_string(), i64::from(prev))]),
        };
        let patch = relay.status_patch(&ready);
        let status = &patch["status"];
        assert_eq!(status["desiredReplicas"], next, "step {i}: desired");
        assert_eq!(status["readyReplicas"], prev, "step {i}: ready");
        assert_eq!(
            status["observedGeneration"], generation,
            "step {i}: generation"
        );
        let phase = if prev >= next { "Ready" } else { "Reconciling" };
        assert_eq!(status["phase"], phase, "step {i}: {prev} -> {next}");
    }

    let relay = Relay::new("relay", spec(1, 3));
    let patch = relay.status_patch(&ReadyFacts {
        ready: HashMap::new(),
    });
    assert_eq!(
        patch["status"]["phase"], "Pending",
        "no ready pods must be Pending"
    );
}

// HANDWRITE-END
