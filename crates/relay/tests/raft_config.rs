// SPEC-MANAGED: tech-design/logic/adopt-raft-host-relaystatemachine-auto-mode-ha-drop-hand-rolled.md#unit-test
// HANDWRITE-BEGIN gap="missing-generator:unit-test:d0cba133" tracker="pending-tracker" reason="Topology smoke (#544): relay derives node id / membership / peer URLs from the standard downward-API quartet via raft_runtime::cluster (the hand-derived local ordinal math is deleted); replica_mode is off without cluster env; RELAY_PEERS overrides peer DNS for a local multi-node group."
//! Topology smoke (#544): relay's cluster shape comes from
//! `raft_runtime::cluster` and the STANDARD downward-API env — the hand-derived
//! ordinal/peer-DNS module is gone, per CONTRIBUTING's "never re-derive the
//! ordinal math locally".
//!
//! isolation: these tests set and remove process-global env vars, so they must
//! not share a process with the tests in `it`.

use std::sync::Mutex;

use raft_runtime::cluster::{replica_mode, ClusterTopology};

// The standard env vars are process-global; serialize the env tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const QUARTET: [(&str, &str); 4] = [
    ("POD_NAME", "relay-1"),
    ("SHARD_COUNT", "1"),
    ("REPLICAS_PER_SHARD", "3"),
    ("VOTER_COUNT", "3"),
];

fn clear_env() {
    for (k, _) in QUARTET {
        std::env::remove_var(k);
    }
    std::env::remove_var("RELAY_PEERS");
}

/// Auto-mode switch: no cluster env (or a single replica) = single-node;
/// REPLICAS_PER_SHARD > 1 = replica/HA.
#[test]
fn replica_mode_is_off_without_cluster_env() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();
    assert!(!replica_mode(), "no env = single-node");
    std::env::set_var("REPLICAS_PER_SHARD", "1");
    assert!(!replica_mode(), "one replica = single-node");
    std::env::set_var("REPLICAS_PER_SHARD", "3");
    assert!(replica_mode(), "scaled out = replica/HA");
    clear_env();
}

/// The serve path's exact derivation: pod relay-1 in a 1-shard/3-replica
/// group is node 1 with voters {0,1,2}; peer URLs use the headless-Service
/// DNS template, and RELAY_PEERS replaces them for a local multi-node group.
#[test]
fn topology_derives_from_standard_env_via_raft_runtime() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();
    for (k, v) in QUARTET {
        std::env::set_var(k, v);
    }

    let t = ClusterTopology::from_env("relay", "relay", 8080, "RELAY_PEERS").unwrap();
    assert_eq!(t.node_id, 1, "node id = replica index from the pod ordinal");
    assert_eq!(t.membership.voters, vec![0, 1, 2]);
    assert!(t.membership.learners.is_empty());
    assert_eq!(t.peers.len(), 2, "self excluded");
    assert_eq!(t.peers[&0], "http://relay-0.relay:8080");
    assert_eq!(t.peers[&2], "http://relay-2.relay:8080");

    // RELAY_PEERS local override replaces the DNS-derived addresses.
    std::env::set_var(
        "RELAY_PEERS",
        "127.0.0.1:7101,127.0.0.1:7102,127.0.0.1:7103",
    );
    let t = ClusterTopology::from_env("relay", "relay", 8080, "RELAY_PEERS").unwrap();
    assert_eq!(t.peers[&0], "http://127.0.0.1:7101");
    assert_eq!(t.peers[&2], "http://127.0.0.1:7103");
    clear_env();
}

#[test]
fn topology_derives_https_scheme_and_peers_override() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();
    for (k, v) in QUARTET {
        std::env::set_var(k, v);
    }

    let t_https =
        ClusterTopology::from_env_with_scheme("relay", "relay", 8080, "RELAY_PEERS", "https")
            .expect("ClusterTopology::from_env_with_scheme with https should succeed");
    assert_eq!(
        t_https.node_id, 1,
        "node_id matches pod ordinal from POD_NAME"
    );
    assert_eq!(
        t_https.membership.voters,
        vec![0, 1, 2],
        "membership.voters contains all voters"
    );
    assert!(
        t_https.membership.learners.is_empty(),
        "membership.learners is empty"
    );
    assert_eq!(t_https.peers.len(), 2, "peers excludes self");
    assert_eq!(
        t_https.peers[&0], "https://relay-0.relay:8080",
        "peers[0] has https scheme"
    );
    assert_eq!(
        t_https.peers[&2], "https://relay-2.relay:8080",
        "peers[2] has https scheme"
    );

    let t_http =
        ClusterTopology::from_env_with_scheme("relay", "relay", 8080, "RELAY_PEERS", "http")
            .expect("ClusterTopology::from_env_with_scheme with http should succeed");
    let t_default = ClusterTopology::from_env("relay", "relay", 8080, "RELAY_PEERS")
        .expect("ClusterTopology::from_env should succeed");
    assert_eq!(
        t_http.node_id, t_default.node_id,
        "node_id matches from_env"
    );
    assert_eq!(
        t_http.membership.voters, t_default.membership.voters,
        "membership.voters matches from_env"
    );
    assert_eq!(
        t_http.membership.learners, t_default.membership.learners,
        "membership.learners matches from_env"
    );
    assert_eq!(t_http.peers, t_default.peers, "peers matches from_env");

    // RELAY_PEERS with the https scheme: the override addresses get the https:// prefix.
    std::env::set_var(
        "RELAY_PEERS",
        "127.0.0.1:7101,127.0.0.1:7102,127.0.0.1:7103",
    );
    let t_override =
        ClusterTopology::from_env_with_scheme("relay", "relay", 8080, "RELAY_PEERS", "https")
            .expect(
                "ClusterTopology::from_env_with_scheme with RELAY_PEERS and https should succeed",
            );
    assert_eq!(
        t_override.peers[&0], "https://127.0.0.1:7101",
        "peers[0] uses RELAY_PEERS with https scheme"
    );
    assert_eq!(
        t_override.peers[&2], "https://127.0.0.1:7103",
        "peers[2] uses RELAY_PEERS with https scheme"
    );

    clear_env();
}

#[test]
fn topology_derives_voter_learner_split() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();
    std::env::set_var("POD_NAME", "relay-2");
    std::env::set_var("SHARD_COUNT", "1");
    std::env::set_var("REPLICAS_PER_SHARD", "3");
    std::env::set_var("VOTER_COUNT", "2");

    let t = ClusterTopology::from_env_with_scheme("relay", "relay", 8080, "RELAY_PEERS", "http")
        .expect("ClusterTopology::from_env_with_scheme with voter/learner split should succeed");
    assert_eq!(t.node_id, 2, "node_id is 2 for pod relay-2 from POD_NAME");
    assert_eq!(
        t.membership.voters,
        vec![0, 1],
        "membership.voters holds [0, 1] when VOTER_COUNT=2"
    );
    assert_eq!(
        t.membership.learners,
        vec![2],
        "membership.learners holds [2] when VOTER_COUNT=2 and REPLICAS_PER_SHARD=3"
    );

    clear_env();
}

#[test]
fn topology_derives_err_on_bad_env() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();

    // Missing POD_NAME
    std::env::set_var("SHARD_COUNT", "1");
    std::env::set_var("REPLICAS_PER_SHARD", "3");
    std::env::set_var("VOTER_COUNT", "3");
    let res_missing_pod =
        ClusterTopology::from_env_with_scheme("relay", "relay", 8080, "RELAY_PEERS", "http");
    assert!(res_missing_pod.is_err(), "missing POD_NAME must return Err");
    let err_msg = res_missing_pod.unwrap_err().to_string();
    assert!(
        err_msg.contains("POD_NAME"),
        "error message '{err_msg}' must name POD_NAME"
    );

    clear_env();

    // Unparsable REPLICAS_PER_SHARD
    std::env::set_var("POD_NAME", "relay-1");
    std::env::set_var("SHARD_COUNT", "1");
    std::env::set_var("REPLICAS_PER_SHARD", "abc");
    std::env::set_var("VOTER_COUNT", "3");
    let res_bad_replicas =
        ClusterTopology::from_env_with_scheme("relay", "relay", 8080, "RELAY_PEERS", "http");
    assert!(
        res_bad_replicas.is_err(),
        "unparsable REPLICAS_PER_SHARD must return Err"
    );
    let err_msg = res_bad_replicas.unwrap_err().to_string();
    assert!(
        err_msg.contains("REPLICAS_PER_SHARD"),
        "error message '{err_msg}' must name REPLICAS_PER_SHARD"
    );

    clear_env();
}

// HANDWRITE-END
