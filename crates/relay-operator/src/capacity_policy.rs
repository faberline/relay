// HANDWRITE-BEGIN gap="missing-generator:logic:capacity-policy" tracker="#47" reason="Pure fake-clock capacity policy: windows, cooldown, arbitration, CR patch."
//! Capacity policy for the `Relay` CR (issue #47). It maps averaged CPU, memory
//! and disk samples plus the current topology to at most one one-step change,
//! and renders that change as a generation-bound merge patch. Times are seconds
//! on the caller's clock; nothing here sleeps or reads the clock.

use serde_json::{json, Value};

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// Watermarks and durations. Percent values are averages over the complete
/// current population.
#[derive(Clone, Debug, PartialEq)]
pub struct CapacityPolicy {
    pub cpu_high: u32,
    pub cpu_low: u32,
    pub memory_high: u32,
    pub memory_low: u32,
    /// Split when disk is strictly above this.
    pub disk_split_bytes: u64,
    /// Merge when disk is strictly below this.
    pub disk_merge_bytes: u64,
    pub scale_out_secs: u64,
    pub scale_in_secs: u64,
    pub disk_merge_secs: u64,
    pub cooldown_secs: u64,
    pub max_sample_gap_secs: u64,
    pub min_replicas: u32,
    pub max_replicas: u32,
    pub min_shards: u32,
    pub max_shards: u32,
}

impl CapacityPolicy {
    pub fn production() -> Self {
        Self {
            cpu_high: 80,
            cpu_low: 30,
            memory_high: 85,
            memory_low: 40,
            disk_split_bytes: 2 * GIB,
            disk_merge_bytes: 256 * MIB,
            scale_out_secs: 120,
            scale_in_secs: 600,
            disk_merge_secs: 1800,
            cooldown_secs: 300,
            max_sample_gap_secs: 30,
            min_replicas: 1,
            max_replicas: 3,
            min_shards: 1,
            max_shards: 3,
        }
    }
}

/// One averaged observation. `None` means the observation was missing or incomplete.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub at_secs: u64,
    pub cpu: Option<u32>,
    pub memory: Option<u32>,
    pub disk_bytes: Option<u64>,
}

/// The current physical topology and whether a previous change is still converging.
#[derive(Clone, Debug, PartialEq)]
pub struct Topology {
    pub shards: u32,
    pub replicas: u32,
    pub pending_generation: bool,
    pub converged_at_secs: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Shards,
    Replicas,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    CpuHigh,
    MemoryHigh,
    CpuMemoryLow,
    DiskHigh,
    DiskLow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldReason {
    PendingGeneration,
    Cooldown,
    StaleOrMissing,
    NoSignal,
    AtBound,
    NoHeadroom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub reason: Reason,
    pub window_start_secs: u64,
    pub window_end_secs: u64,
    pub watermark: u64,
    pub observed: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Hold(HoldReason),
    Change {
        axis: Axis,
        direction: Direction,
        desired_shards: u32,
        desired_replicas: u32,
        evidence: Evidence,
    },
}

/// Decides at most one one-step change. `samples` must be sorted by `at_secs`.
///
/// Arbitration, first qualifying and allowed candidate wins: shard split, replica
/// growth, replica shrink, shard merge.
pub fn decide(
    policy: &CapacityPolicy,
    topology: &Topology,
    samples: &[Sample],
    now_secs: u64,
) -> Decision {
    if topology.pending_generation {
        return Decision::Hold(HoldReason::PendingGeneration);
    }
    let mut floor = 0;
    if let Some(converged) = topology.converged_at_secs {
        let cooldown_end = converged + policy.cooldown_secs;
        if now_secs < cooldown_end {
            return Decision::Hold(HoldReason::Cooldown);
        }
        floor = cooldown_end;
    }
    let usable: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.at_secs >= floor && s.at_secs <= now_secs)
        .collect();
    let gap = policy.max_sample_gap_secs;
    let mut bound_hit = false;
    let mut headroom_hit = false;

    // 1. Shard split: disk strictly above the split watermark.
    if let Some((start, newest)) = window(
        &usable,
        now_secs,
        gap,
        |s| s.disk_bytes.is_some_and(|d| d > policy.disk_split_bytes),
        policy.scale_out_secs,
    ) {
        if topology.shards < policy.max_shards {
            return change(
                topology,
                Axis::Shards,
                Direction::Up,
                evidence(policy, Reason::DiskHigh, start, now_secs, newest),
            );
        }
        bound_hit = true;
    }

    // 2. Replica growth: CPU first, then memory.
    let growth = window(
        &usable,
        now_secs,
        gap,
        |s| s.cpu.is_some_and(|c| c > policy.cpu_high),
        policy.scale_out_secs,
    )
    .map(|w| (Reason::CpuHigh, w))
    .or_else(|| {
        window(
            &usable,
            now_secs,
            gap,
            |s| s.memory.is_some_and(|m| m > policy.memory_high),
            policy.scale_out_secs,
        )
        .map(|w| (Reason::MemoryHigh, w))
    });
    if let Some((reason, (start, newest))) = growth {
        if topology.replicas < policy.max_replicas {
            return change(
                topology,
                Axis::Replicas,
                Direction::Up,
                evidence(policy, reason, start, now_secs, newest),
            );
        }
        bound_hit = true;
    }

    // 3. Replica shrink: CPU and memory both below their low watermarks.
    if let Some((start, newest)) = window(
        &usable,
        now_secs,
        gap,
        |s| {
            matches!((s.cpu, s.memory), (Some(c), Some(m))
                if c < policy.cpu_low && m < policy.memory_low)
        },
        policy.scale_in_secs,
    ) {
        if topology.replicas > policy.min_replicas {
            return change(
                topology,
                Axis::Replicas,
                Direction::Down,
                evidence(policy, Reason::CpuMemoryLow, start, now_secs, newest),
            );
        }
        bound_hit = true;
    }

    // 4. Shard merge: disk strictly below the merge watermark, with headroom.
    if let Some((start, newest)) = window(
        &usable,
        now_secs,
        gap,
        |s| s.disk_bytes.is_some_and(|d| d < policy.disk_merge_bytes),
        policy.disk_merge_secs,
    ) {
        if topology.shards > policy.min_shards.max(1) {
            let disk = u128::from(newest.disk_bytes.unwrap_or_default());
            let shards = u128::from(topology.shards);
            let projected = disk * shards / (shards - 1);
            if projected < u128::from(policy.disk_split_bytes) {
                return change(
                    topology,
                    Axis::Shards,
                    Direction::Down,
                    evidence(policy, Reason::DiskLow, start, now_secs, newest),
                );
            }
            headroom_hit = true;
        } else {
            bound_hit = true;
        }
    }

    if bound_hit {
        return Decision::Hold(HoldReason::AtBound);
    }
    if headroom_hit {
        return Decision::Hold(HoldReason::NoHeadroom);
    }
    let stale = match usable.last() {
        None => true,
        Some(newest) => {
            now_secs.saturating_sub(newest.at_secs) > gap
                || newest.cpu.is_none()
                || newest.memory.is_none()
                || newest.disk_bytes.is_none()
        }
    };
    if stale {
        Decision::Hold(HoldReason::StaleOrMissing)
    } else {
        Decision::Hold(HoldReason::NoSignal)
    }
}

/// The start of the run of samples that satisfy `pred`, ending at the newest usable
/// sample, and that run's newest sample. `None` when the newest sample is stale or
/// fails `pred`, or when the run is shorter than `duration` (equal qualifies).
fn window<'a>(
    usable: &[&'a Sample],
    now_secs: u64,
    max_gap: u64,
    pred: impl Fn(&Sample) -> bool,
    duration: u64,
) -> Option<(u64, &'a Sample)> {
    let newest = *usable.last()?;
    if now_secs.saturating_sub(newest.at_secs) > max_gap || !pred(newest) {
        return None;
    }
    let mut start = newest.at_secs;
    for pair in usable.windows(2).rev() {
        let (older, newer) = (pair[0], pair[1]);
        if newer.at_secs - older.at_secs > max_gap || !pred(older) {
            break;
        }
        start = older.at_secs;
    }
    (now_secs.saturating_sub(start) >= duration).then_some((start, newest))
}

fn evidence(
    policy: &CapacityPolicy,
    reason: Reason,
    window_start_secs: u64,
    window_end_secs: u64,
    newest: &Sample,
) -> Evidence {
    let (watermark, observed) = match reason {
        Reason::CpuHigh => (
            u64::from(policy.cpu_high),
            u64::from(newest.cpu.unwrap_or_default()),
        ),
        Reason::MemoryHigh => (
            u64::from(policy.memory_high),
            u64::from(newest.memory.unwrap_or_default()),
        ),
        Reason::CpuMemoryLow => (
            u64::from(policy.cpu_low),
            u64::from(newest.cpu.unwrap_or_default()),
        ),
        Reason::DiskHigh => (
            policy.disk_split_bytes,
            newest.disk_bytes.unwrap_or_default(),
        ),
        Reason::DiskLow => (
            policy.disk_merge_bytes,
            newest.disk_bytes.unwrap_or_default(),
        ),
    };
    Evidence {
        reason,
        window_start_secs,
        window_end_secs,
        watermark,
        observed,
    }
}

fn change(topology: &Topology, axis: Axis, direction: Direction, evidence: Evidence) -> Decision {
    let (desired_shards, desired_replicas) = match (axis, direction) {
        (Axis::Shards, Direction::Up) => (topology.shards + 1, topology.replicas),
        (Axis::Shards, Direction::Down) => (topology.shards.saturating_sub(1), topology.replicas),
        (Axis::Replicas, Direction::Up) => (topology.shards, topology.replicas + 1),
        (Axis::Replicas, Direction::Down) => (topology.shards, topology.replicas.saturating_sub(1)),
    };
    Decision::Change {
        axis,
        direction,
        desired_shards,
        desired_replicas,
        evidence,
    }
}

/// The CR merge patch for a change, or `None` for a hold. The patch is pure: the
/// same decision and generation always give an equal value.
pub fn cr_patch(decision: &Decision, generation: i64) -> Option<Value> {
    let Decision::Change {
        axis,
        direction,
        desired_shards,
        desired_replicas,
        evidence,
    } = decision
    else {
        return None;
    };
    let (axis_name, spec) = match axis {
        Axis::Shards => ("shards", json!({ "shardCount": desired_shards })),
        Axis::Replicas => (
            "replicas",
            json!({ "replicasPerShard": desired_replicas, "voterCount": desired_replicas }),
        ),
    };
    let direction_name = match direction {
        Direction::Up => "up",
        Direction::Down => "down",
    };
    let reason_name = match evidence.reason {
        Reason::CpuHigh => "cpu-high",
        Reason::MemoryHigh => "memory-high",
        Reason::CpuMemoryLow => "cpu-memory-low",
        Reason::DiskHigh => "disk-high",
        Reason::DiskLow => "disk-low",
    };
    let annotation = json!({
        "generation": generation,
        "axis": axis_name,
        "direction": direction_name,
        "reason": reason_name,
        "windowStartSecs": evidence.window_start_secs,
        "windowEndSecs": evidence.window_end_secs,
        "watermark": evidence.watermark,
        "observed": evidence.observed,
    });
    Some(json!({
        "metadata": { "annotations": { "relay.dev/capacity-decision": annotation.to_string() } },
        "spec": spec,
    }))
}
// HANDWRITE-END
