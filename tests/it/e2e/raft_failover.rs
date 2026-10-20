//! Suite `raft_failover`: three relay voters keep every committed publish
//! through the loss of their leader. It replaces scripts/kind-failover-smoke.sh.
//!
//! The deploy is k8s/overlays/gke-test: StatefulSet/relay with three voters
//! (SHARD_COUNT 1, REPLICAS_PER_SHARD 3, VOTER_COUNT 3), Service/relay at
//! `E2E_ENDPOINT`, and the headless Service relay-headless that gives each
//! pod `http://relay-<n>.relay-headless:7000`. The suite reads each voter's
//! `/raftz`, publishes through the Service (a follower forwards to the
//! leader), deletes the leader pod, and checks that a new leader takes over,
//! the replacement pod rejoins, and every voter holds both publishes.

use std::time::Duration;

use serde_json::{json, Value};

use super::{
    assert_pods_run, endpoint, evidence, kubectl, try_kubectl_json, wait_for, Expected, Http,
};

const SUITE: &str = "raft_failover";
const SERVER_LABEL: &str = "app=relay,role=server";
const VOTERS: usize = 3;
const SUBJECT: &str = "e2e-raft-failover";
/// Published before the leader dies; must survive it.
const BEFORE: &str = "before-failover";
/// Published after the new leader takes over.
const AFTER: &str = "after-failover";

fn pod_name(ordinal: usize) -> String {
    format!("relay-{ordinal}")
}

fn pod_url(ordinal: usize) -> String {
    format!("http://relay-{ordinal}.relay-headless:7000")
}

/// `/raftz` of every voter in `ordinals`, in that order.
fn raftz(http: &Http, ordinals: &[usize]) -> Result<Vec<Value>, String> {
    ordinals
        .iter()
        .map(|&ordinal| http.get_json(&format!("{}/raftz", pod_url(ordinal))))
        .collect()
}

/// The ordinal (index into `ordinals`) and term of the one leader that every
/// status in `statuses` follows: exactly one reports `is_leader`, all name it
/// as `leader`, and all are in its term. Otherwise the disagreement.
fn settled_leader(ordinals: &[usize], statuses: &[Value]) -> Result<(usize, u64), String> {
    let leaders: Vec<usize> = statuses
        .iter()
        .enumerate()
        .filter(|(_, status)| status["is_leader"] == true)
        .map(|(at, _)| at)
        .collect();
    let [at] = leaders[..] else {
        return Err(format!(
            "{} voters claim leadership: {}",
            leaders.len(),
            summary(ordinals, statuses)
        ));
    };
    let leader = &statuses[at];
    let term = leader["term"]
        .as_u64()
        .ok_or_else(|| format!("leader has no term: {leader}"))?;
    let agreed = statuses
        .iter()
        .all(|status| status["leader"] == leader["id"] && status["term"] == term);
    if !agreed {
        return Err(format!(
            "voters disagree on the leader or term: {}",
            summary(ordinals, statuses)
        ));
    }
    Ok((ordinals[at], term))
}

/// One line per voter for failure messages and evidence.
fn summary(ordinals: &[usize], statuses: &[Value]) -> Value {
    ordinals
        .iter()
        .zip(statuses)
        .map(|(&ordinal, status)| {
            json!({
                "pod": pod_name(ordinal),
                "id": status["id"],
                "role": status["role"],
                "term": status["term"],
                "leader": status["leader"],
                "commit_index": status["commit_index"],
                "applied_index": status["applied_index"],
                "durability_error": status["durability_error"],
            })
        })
        .collect()
}

/// Publishes `message_id` through the Service. A retry of the same id is
/// idempotent, so a 503 while raft has no leader is retried.
fn publish(http: &Http, endpoint: &str, message_id: &str) -> Value {
    let url = format!("{endpoint}/v1/{SUBJECT}/publish");
    let body = json!({ "message_id": message_id, "payload": { "suite": SUITE } });
    wait_for(
        &format!("publish {message_id}"),
        Duration::from_secs(60),
        || http.post_json(&url, &body),
    )
}

/// Every voter holds exactly `want` entries of [`SUBJECT`] in its own engine
/// and reports no durability error. Returns the per-voter state.
fn replicated(http: &Http, want: u64) -> Result<Value, String> {
    let ordinals: Vec<usize> = (0..VOTERS).collect();
    let statuses = raftz(http, &ordinals)?;
    let mut lens = Vec::with_capacity(VOTERS);
    for &ordinal in &ordinals {
        let len = http.get_json(&format!("{}/v1/{SUBJECT}/len", pod_url(ordinal)))?;
        let len = len["latest_seq"]
            .as_u64()
            .ok_or_else(|| format!("{}: no latest_seq in {len}", pod_name(ordinal)))?;
        lens.push(len);
    }
    let state = json!({ "log_len": lens, "voters": summary(&ordinals, &statuses) });
    if lens.iter().any(|&len| len != want) {
        return Err(format!("want {want} entries on every voter: {state}"));
    }
    if statuses
        .iter()
        .any(|status| !status["durability_error"].is_null())
    {
        return Err(format!("a voter reports a durability error: {state}"));
    }
    Ok(state)
}

/// The pod's UID once it is Ready, or its current state.
fn ready_uid(ordinal: usize) -> Result<String, String> {
    let pod = try_kubectl_json(&["get", "pod", &pod_name(ordinal)])?;
    let uid = pod["metadata"]["uid"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let ready = pod["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|condition| condition["type"] == "Ready" && condition["status"] == "True");
    if ready && pod["metadata"]["deletionTimestamp"].is_null() {
        Ok(uid)
    } else {
        Err(format!(
            "{} uid {uid} not Ready: {}",
            pod_name(ordinal),
            pod["status"]
        ))
    }
}

fn assert_identity(expected: &Expected) {
    let image_ids = assert_pods_run(expected, SERVER_LABEL, VOTERS as u64);
    let version = kubectl(&["exec", &pod_name(0), "--", "relay", "--version"]);
    let version = version.trim();
    assert_eq!(
        version,
        format!("relay {}", expected.version),
        "relay --version in {}",
        pod_name(0)
    );
    evidence(
        SUITE,
        "identity",
        json!({
            "image": expected.image,
            "runtime_digest": expected.runtime_digest,
            "git_sha": expected.git_sha,
            "version": version,
            "server_image_ids": image_ids,
        }),
    );
}

#[test]
#[ignore = "e2e: needs E2E_ENDPOINT"]
fn leader_loss_keeps_committed_publishes() {
    let endpoint = endpoint();
    let expected = Expected::from_env();
    let http = Http::new();
    assert_identity(&expected);

    let all: Vec<usize> = (0..VOTERS).collect();
    let (leader, term) = wait_for(
        "one leader that all three voters follow",
        Duration::from_secs(300),
        || settled_leader(&all, &raftz(&http, &all)?),
    );
    evidence(
        SUITE,
        "leader_elected",
        json!({ "leader": pod_name(leader), "term": term }),
    );

    let before = publish(&http, &endpoint, BEFORE);
    let before_seq = before["seq"].clone();
    let state = wait_for(
        "every voter holds the first publish",
        Duration::from_secs(120),
        || replicated(&http, 1),
    );
    evidence(
        SUITE,
        "replicated_before_failover",
        json!({ "message_id": BEFORE, "outcome": before, "state": state }),
    );

    let old_uid = ready_uid(leader).unwrap_or_else(|state| panic!("leader pod: {state}"));
    kubectl(&[
        "delete",
        "pod",
        &pod_name(leader),
        "--grace-period=1",
        "--wait=false",
    ]);
    evidence(
        SUITE,
        "leader_deleted",
        json!({ "pod": pod_name(leader), "uid": old_uid, "term": term }),
    );

    let survivors: Vec<usize> = all.iter().copied().filter(|&o| o != leader).collect();
    let (new_leader, new_term) = wait_for(
        "a new leader among the two survivors",
        Duration::from_secs(180),
        || {
            let (ordinal, new_term) = settled_leader(&survivors, &raftz(&http, &survivors)?)?;
            if new_term > term {
                Ok((ordinal, new_term))
            } else {
                Err(format!(
                    "survivors still in term {new_term}, not past {term}"
                ))
            }
        },
    );
    evidence(
        SUITE,
        "leader_reelected",
        json!({ "leader": pod_name(new_leader), "term": new_term, "previous_term": term }),
    );

    let new_uid = wait_for(
        "the deleted leader's pod is replaced and Ready",
        Duration::from_secs(600),
        || {
            let uid = ready_uid(leader)?;
            if uid == old_uid {
                Err(format!("{} still runs uid {uid}", pod_name(leader)))
            } else {
                Ok(uid)
            }
        },
    );
    let (leader_now, term_now) = wait_for(
        "all three voters follow one leader again",
        Duration::from_secs(300),
        || settled_leader(&all, &raftz(&http, &all)?),
    );
    evidence(
        SUITE,
        "replacement_rejoined",
        json!({
            "pod": pod_name(leader),
            "old_uid": old_uid,
            "new_uid": new_uid,
            "leader": pod_name(leader_now),
            "term": term_now,
        }),
    );

    let after = publish(&http, &endpoint, AFTER);
    let replay = publish(&http, &endpoint, BEFORE);
    assert_eq!(
        replay["deduped"], true,
        "{BEFORE} published again after the failover must be a dedupe: {replay}"
    );
    assert_eq!(
        replay["seq"], before_seq,
        "{BEFORE} kept its seq through the failover"
    );
    assert_ne!(after["seq"], before_seq, "{AFTER} got its own seq: {after}");
    let state = wait_for(
        "every voter holds both publishes",
        Duration::from_secs(300),
        || replicated(&http, 2),
    );
    evidence(
        SUITE,
        "no_committed_loss",
        json!({
            "before": before,
            "after": after,
            "replay_of_before": replay,
            "state": state,
        }),
    );
}

#[test]
fn settled_leader_needs_one_leader_that_every_voter_follows() {
    let status = |id: u64, leader: u64, term: u64| json!({ "id": id, "is_leader": id == leader, "leader": leader, "term": term });
    let ordinals = [0, 1, 2];
    assert_eq!(
        settled_leader(
            &ordinals,
            &[status(1, 2, 4), status(2, 2, 4), status(3, 2, 4)]
        ),
        Ok((1, 4))
    );
    // A voter still in the old term, or following another leader.
    assert!(settled_leader(
        &ordinals,
        &[status(1, 2, 4), status(2, 2, 4), status(3, 2, 3)]
    )
    .is_err());
    assert!(settled_leader(
        &ordinals,
        &[status(1, 2, 4), status(2, 2, 4), status(3, 1, 4)]
    )
    .is_err());
    // No leader, and two leaders.
    let none = json!({ "id": 1, "is_leader": false, "leader": null, "term": 4 });
    assert!(settled_leader(&ordinals, &[none.clone(), none.clone(), none]).is_err());
    assert!(settled_leader(&[0, 1], &[status(1, 1, 4), status(2, 2, 4)]).is_err());
    // Survivors keep their own ordinals.
    assert_eq!(
        settled_leader(&[0, 2], &[status(1, 3, 5), status(3, 3, 5)]),
        Ok((2, 5))
    );
}
