// HANDWRITE-BEGIN gap="missing-generator:unit-test:capacity-policy" tracker="#47" reason="Fake-clock tests for the pure capacity policy: exact windows, cooldown, arbitration, CR patch."
//! Fake-clock tests for `relay::operator::capacity_policy` (issue #47). Every
//! duration is driven by `now_secs`; nothing sleeps.
#![cfg(feature = "operator")]

use relay::operator::capacity_policy::*;
use serde_json::Value;

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// One sample every `step` seconds from `from` to `to`, both ends included.
fn series(
    from: u64,
    to: u64,
    step: u64,
    cpu: Option<u32>,
    mem: Option<u32>,
    disk: Option<u64>,
) -> Vec<Sample> {
    (from..=to)
        .step_by(step as usize)
        .map(|at_secs| Sample {
            at_secs,
            cpu,
            memory: mem,
            disk_bytes: disk,
        })
        .collect()
}

fn topo(shards: u32, replicas: u32) -> Topology {
    Topology {
        shards,
        replicas,
        pending_generation: false,
        converged_at_secs: None,
    }
}

/// Checks that a change moves exactly one shard or one replica layer by one.
fn assert_one_step(topology: &Topology, decision: &Decision) {
    let Decision::Change {
        desired_shards,
        desired_replicas,
        ..
    } = decision
    else {
        panic!("expected a change, got {decision:?}");
    };
    let shard_moves = desired_shards.abs_diff(topology.shards);
    let replica_moves = desired_replicas.abs_diff(topology.replicas);
    assert!(
        (shard_moves == 1 && replica_moves == 0) || (shard_moves == 0 && replica_moves == 1),
        "exactly one one-step move: {decision:?}"
    );
}

#[test]
fn cpu_high_needs_exactly_120s() {
    let policy = CapacityPolicy::production();

    let short = series(0, 90, 30, Some(90), Some(60), Some(GIB));
    assert_eq!(
        decide(&policy, &topo(1, 1), &short, 90),
        Decision::Hold(HoldReason::NoSignal)
    );

    let full = series(0, 120, 30, Some(90), Some(60), Some(GIB));
    assert_eq!(
        decide(&policy, &topo(1, 1), &full, 120),
        Decision::Change {
            axis: Axis::Replicas,
            direction: Direction::Up,
            desired_shards: 1,
            desired_replicas: 2,
            evidence: Evidence {
                reason: Reason::CpuHigh,
                window_start_secs: 0,
                window_end_secs: 120,
                watermark: 80,
                observed: 90,
            },
        }
    );

    // Exactly the watermark is not above it, so it never qualifies.
    let at_watermark = series(0, 600, 30, Some(80), Some(60), Some(GIB));
    assert_eq!(
        decide(&policy, &topo(1, 1), &at_watermark, 600),
        Decision::Hold(HoldReason::NoSignal)
    );
}

#[test]
fn memory_high_requests_one_layer() {
    let policy = CapacityPolicy::production();
    let samples = series(0, 120, 30, Some(50), Some(95), Some(GIB));

    let decision = decide(&policy, &topo(1, 1), &samples, 120);
    assert!(matches!(
        decision,
        Decision::Change {
            axis: Axis::Replicas,
            direction: Direction::Up,
            evidence: Evidence {
                reason: Reason::MemoryHigh,
                ..
            },
            ..
        }
    ));
    assert_one_step(&topo(1, 1), &decision);

    assert_eq!(
        decide(&policy, &topo(1, 3), &samples, 120),
        Decision::Hold(HoldReason::AtBound)
    );
}

#[test]
fn scale_in_needs_exactly_600s_and_both_low() {
    let policy = CapacityPolicy::production();

    let short = series(0, 570, 30, Some(20), Some(30), Some(GIB));
    assert_eq!(
        decide(&policy, &topo(1, 2), &short, 570),
        Decision::Hold(HoldReason::NoSignal)
    );

    let full = series(0, 600, 30, Some(20), Some(30), Some(GIB));
    assert_eq!(
        decide(&policy, &topo(1, 2), &full, 600),
        Decision::Change {
            axis: Axis::Replicas,
            direction: Direction::Down,
            desired_shards: 1,
            desired_replicas: 1,
            evidence: Evidence {
                reason: Reason::CpuMemoryLow,
                window_start_secs: 0,
                window_end_secs: 600,
                watermark: 30,
                observed: 20,
            },
        }
    );

    // Memory is not low, so CPU alone does not shrink.
    let cpu_only = series(0, 600, 30, Some(20), Some(60), Some(GIB));
    assert_eq!(
        decide(&policy, &topo(1, 2), &cpu_only, 600),
        Decision::Hold(HoldReason::NoSignal)
    );

    assert_eq!(
        decide(&policy, &topo(1, 1), &full, 600),
        Decision::Hold(HoldReason::AtBound)
    );
}

#[test]
fn disk_split_is_strict_and_one_shard() {
    let policy = CapacityPolicy::production();

    let at_watermark = series(0, 600, 30, Some(50), Some(60), Some(2 * GIB));
    assert_eq!(
        decide(&policy, &topo(1, 1), &at_watermark, 600),
        Decision::Hold(HoldReason::NoSignal)
    );

    let above = series(0, 120, 30, Some(50), Some(60), Some(2 * GIB + 1));
    assert_eq!(
        decide(&policy, &topo(1, 1), &above, 120),
        Decision::Change {
            axis: Axis::Shards,
            direction: Direction::Up,
            desired_shards: 2,
            desired_replicas: 1,
            evidence: Evidence {
                reason: Reason::DiskHigh,
                window_start_secs: 0,
                window_end_secs: 120,
                watermark: 2 * GIB,
                observed: 2 * GIB + 1,
            },
        }
    );

    assert_eq!(
        decide(&policy, &topo(3, 1), &above, 120),
        Decision::Hold(HoldReason::AtBound)
    );
}

#[test]
fn disk_merge_needs_1800s_and_headroom() {
    let policy = CapacityPolicy::production();

    let short = series(0, 1770, 30, Some(50), Some(60), Some(100 * MIB));
    assert_eq!(
        decide(&policy, &topo(2, 1), &short, 1770),
        Decision::Hold(HoldReason::NoSignal)
    );

    let full = series(0, 1800, 30, Some(50), Some(60), Some(100 * MIB));
    assert_eq!(
        decide(&policy, &topo(2, 1), &full, 1800),
        Decision::Change {
            axis: Axis::Shards,
            direction: Direction::Down,
            desired_shards: 1,
            desired_replicas: 1,
            evidence: Evidence {
                reason: Reason::DiskLow,
                window_start_secs: 0,
                window_end_secs: 1800,
                watermark: 256 * MIB,
                observed: 100 * MIB,
            },
        }
    );

    // Projected post-merge disk is 200 MiB, which is not below a 150 MiB split.
    let mut tight = policy.clone();
    tight.disk_split_bytes = 150 * MIB;
    assert_eq!(
        decide(&tight, &topo(2, 1), &full, 1800),
        Decision::Hold(HoldReason::NoHeadroom)
    );

    assert_eq!(
        decide(&policy, &topo(1, 1), &full, 1800),
        Decision::Hold(HoldReason::AtBound)
    );
}

#[test]
fn missing_or_stale_samples_hold() {
    let policy = CapacityPolicy::production();

    // (a) A missing CPU sample restarts the window: only 60 s of high CPU remain.
    let mut gap = series(0, 60, 30, Some(90), Some(60), Some(GIB));
    gap.push(Sample {
        at_secs: 90,
        cpu: None,
        memory: Some(60),
        disk_bytes: Some(GIB),
    });
    gap.extend(series(120, 180, 30, Some(90), Some(60), Some(GIB)));
    assert_eq!(
        decide(&policy, &topo(1, 1), &gap, 180),
        Decision::Hold(HoldReason::NoSignal)
    );

    // (b) The newest sample has a missing metric.
    let mut none_newest = series(0, 120, 30, Some(90), Some(60), Some(GIB));
    none_newest.last_mut().expect("samples").cpu = None;
    assert_eq!(
        decide(&policy, &topo(1, 1), &none_newest, 120),
        Decision::Hold(HoldReason::StaleOrMissing)
    );

    // (c) Samples 60 s apart break the window (the gap is above 30 s).
    let sparse: Vec<Sample> = [0, 60, 120]
        .into_iter()
        .map(|at_secs| Sample {
            at_secs,
            cpu: Some(90),
            memory: Some(60),
            disk_bytes: Some(GIB),
        })
        .collect();
    assert!(!matches!(
        decide(&policy, &topo(1, 1), &sparse, 120),
        Decision::Change { .. }
    ));

    // (d) The newest sample is 80 s old.
    let stale = series(0, 120, 30, Some(90), Some(60), Some(GIB));
    assert_eq!(
        decide(&policy, &topo(1, 1), &stale, 200),
        Decision::Hold(HoldReason::StaleOrMissing)
    );

    // (e) No samples at all.
    assert_eq!(
        decide(&policy, &topo(1, 1), &[], 100),
        Decision::Hold(HoldReason::StaleOrMissing)
    );
}

#[test]
fn deadband_holds() {
    let policy = CapacityPolicy::production();
    let samples = series(0, 2000, 30, Some(50), Some(60), Some(GIB));
    assert_eq!(
        decide(&policy, &topo(1, 1), &samples, 2000),
        Decision::Hold(HoldReason::NoSignal)
    );
}

#[test]
fn pending_generation_and_cooldown_hold() {
    let policy = CapacityPolicy::production();
    let qualifying = series(0, 120, 30, Some(90), Some(60), Some(GIB));

    let pending = Topology {
        pending_generation: true,
        ..topo(1, 1)
    };
    assert_eq!(
        decide(&policy, &pending, &qualifying, 120),
        Decision::Hold(HoldReason::PendingGeneration)
    );

    let cooling = Topology {
        converged_at_secs: Some(1000),
        ..topo(1, 1)
    };
    let before_end = series(1000, 1299, 30, Some(90), Some(60), Some(GIB));
    assert_eq!(
        decide(&policy, &cooling, &before_end, 1299),
        Decision::Hold(HoldReason::Cooldown)
    );

    // At the cooldown end, the window starts at 1300 and has no length yet.
    let at_end = series(1000, 1300, 30, Some(90), Some(60), Some(GIB));
    assert_eq!(
        decide(&policy, &cooling, &at_end, 1300),
        Decision::Hold(HoldReason::NoSignal)
    );
}

#[test]
fn cooldown_discards_old_samples_and_needs_a_new_window() {
    let policy = CapacityPolicy::production();
    let cooled = Topology {
        converged_at_secs: Some(1000),
        ..topo(1, 1)
    };
    // Step 10 so that a sample lands on the cooldown end, 1300.
    let samples = series(1000, 1420, 10, Some(90), Some(60), Some(GIB));

    assert_eq!(
        decide(&policy, &cooled, &samples, 1410),
        Decision::Hold(HoldReason::NoSignal)
    );
    assert_eq!(
        decide(&policy, &cooled, &samples, 1420),
        Decision::Change {
            axis: Axis::Replicas,
            direction: Direction::Up,
            desired_shards: 1,
            desired_replicas: 2,
            evidence: Evidence {
                reason: Reason::CpuHigh,
                window_start_secs: 1300,
                window_end_secs: 1420,
                watermark: 80,
                observed: 90,
            },
        }
    );
}

#[test]
fn arbitration_is_deterministic_one_axis_one_step() {
    let policy = CapacityPolicy::production();

    // Split beats replica growth.
    let split_vs_growth = series(0, 120, 30, Some(90), Some(50), Some(2 * GIB + 1));
    let decision = decide(&policy, &topo(1, 1), &split_vs_growth, 120);
    assert!(matches!(
        decision,
        Decision::Change {
            axis: Axis::Shards,
            direction: Direction::Up,
            ..
        }
    ));
    assert_one_step(&topo(1, 1), &decision);

    // Expansion beats contraction (merge).
    let growth_vs_merge = series(0, 1800, 30, Some(90), Some(50), Some(100 * MIB));
    let decision = decide(&policy, &topo(2, 1), &growth_vs_merge, 1800);
    assert!(matches!(
        decision,
        Decision::Change {
            axis: Axis::Replicas,
            direction: Direction::Up,
            ..
        }
    ));
    assert_one_step(&topo(2, 1), &decision);

    // Replica shrink beats shard merge.
    let shrink_vs_merge = series(0, 1800, 30, Some(20), Some(30), Some(100 * MIB));
    let decision = decide(&policy, &topo(2, 2), &shrink_vs_merge, 1800);
    assert!(matches!(
        decision,
        Decision::Change {
            axis: Axis::Replicas,
            direction: Direction::Down,
            ..
        }
    ));
    assert_one_step(&topo(2, 2), &decision);
}

#[test]
fn cr_patch_is_generation_bound_and_idempotent() {
    let policy = CapacityPolicy::production();
    let samples = series(0, 120, 30, Some(90), Some(60), Some(GIB));
    let decision = decide(&policy, &topo(1, 1), &samples, 120);

    let patch = cr_patch(&decision, 7).expect("a change has a patch");
    assert_eq!(cr_patch(&decision, 7), Some(patch.clone()));
    assert_eq!(patch["spec"]["replicasPerShard"], 2);
    assert_eq!(patch["spec"]["voterCount"], 2);
    assert!(patch["spec"].get("shardCount").is_none());

    let text = patch["metadata"]["annotations"]["relay.dev/capacity-decision"]
        .as_str()
        .expect("annotation text");
    let annotation: Value = serde_json::from_str(text).expect("annotation is JSON");
    assert_eq!(annotation["generation"], 7);
    assert_eq!(annotation["axis"], "replicas");
    assert_eq!(annotation["direction"], "up");
    assert_eq!(annotation["reason"], "cpu-high");
    assert_eq!(annotation["windowStartSecs"], 0);
    assert_eq!(annotation["windowEndSecs"], 120);
    assert_eq!(annotation["watermark"], 80);
    assert_eq!(annotation["observed"], 90);

    assert_ne!(cr_patch(&decision, 8), Some(patch));

    let split_samples = series(0, 120, 30, Some(50), Some(60), Some(2 * GIB + 1));
    let split = decide(&policy, &topo(1, 1), &split_samples, 120);
    let split_patch = cr_patch(&split, 7).expect("a split has a patch");
    assert_eq!(split_patch["spec"]["shardCount"], 2);
    assert!(split_patch["spec"].get("replicasPerShard").is_none());

    assert_eq!(cr_patch(&Decision::Hold(HoldReason::NoSignal), 7), None);
}

// Negative: a second change cannot be accepted while the first generation is still
// converging. Without the pending-generation hold, this would decide a second step.
#[test]
fn no_decision_survives_a_second_step() {
    let policy = CapacityPolicy::production();
    let samples = series(0, 120, 30, Some(90), Some(60), Some(GIB));

    let first = decide(&policy, &topo(1, 1), &samples, 120);
    assert!(matches!(first, Decision::Change { .. }));

    let pending = Topology {
        pending_generation: true,
        ..topo(1, 1)
    };
    assert_eq!(
        decide(&policy, &pending, &samples, 120),
        Decision::Hold(HoldReason::PendingGeneration)
    );
}

// HANDWRITE-END
