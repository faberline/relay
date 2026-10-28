# Restart, Failover, and Live Soak Verification Evidence

This document records the committed gate definition, wrapper implementation, and cloud qualification evidence for the "restart, failover, and live soak" verification axis under parent epic `faberline/axiom#2320`.

## Metadata

- **Base Commit**: `11a238db1723ee850e078885effe9926cbece888` (origin/main)
- **Candidate Branch**: `issue/48`
- **Associated PR**: [#103](https://github.com/faberline/relay/pull/103)
- **Environment**: Cloud CI (`x86_64-unknown-linux-gnu`, GitHub Actions runner `ubuntu-latest`)
- **Capability Axis**: `cross-service-delivery-verification` / `relay-restart-failover-live-soak-verification`
- **Terminal Qualification**: `PENDING` (awaiting candidate head CI execution and uploaded evidence receipt verification)

## Verification Command and Gate Structure

The reproducible integration gate is wrapped in [scripts/verify-restart-failover-soak.sh](../../scripts/verify-restart-failover-soak.sh). It executes in cloud CI on an ordinary unprivileged GitHub Actions runner:

1. **Step 1 (Module Integration)**:
   ```bash
   cargo test --locked -p relay --test it -- durable:: raft_cluster:: raft_persistence:: backup::
   ```
2. **Step 2 (Non-Ignored Bounded Soak Harness)**:
   ```bash
   cargo test --locked -p relay --test it -- --ignored stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak
   ```

The wrapper asserts non-zero executed test counts for both steps, validates that every required module reports passing tests, verifies that `scripts/soak.sh` completes with 0 errors and non-zero operations within pinned resource ceilings, and writes raw logs and a structured JSON receipt (`target/restart-failover-soak-evidence/receipt.json`) before temporary directory cleanup.

## Test Inventory and Execution Provenance

### Inventory of Compiled Test Targets (16 Tests in `crates/relay/tests/it`)

- `durable::` (6 tests compiled/listed)
- `raft_cluster::` (3 tests compiled/listed)
- `raft_persistence::` (2 tests compiled/listed)
- `backup::` (4 tests compiled/listed)
- `stability_relay_competitor_performance_bounded_soak::` (1 test compiled/listed, marked `#[ignore]`)

### Verified Cloud CI Execution Status (Candidate Head `834b6a00e56ef7445eeb5c7789a329feb57504ff`)

From verified cloud CI run [`40133268456`](https://github.com/faberline/relay/actions/runs/40133268456) (job `120989346555`, artifact `12575669906`):

| Test Module / Case | Compiled Target Present | Cloud CI Status (Run 40133268456) |
|--------------------|-------------------------|-----------------------------------|
| `durable::` (6 cases) | Yes | **6 Passed** |
| `raft_cluster::` (3 cases) | Yes | **3 Passed** |
| `raft_persistence::` (2 cases) | Yes | **2 Passed** |
| `backup::` (4 cases) | Yes | **4 Passed** |
| `stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak` | Yes | **1 Passed** (Step 2 non-ignored soak gate) |

All 15 standard integration cases and the bounded soak test executed and passed (total 16 passed, 0 failed, 0 ignored).

### Bounded Soak Two-Window Resource Metrics

The soak workload assertions enforce the following frozen thresholds:
- **Duration**: 60 seconds (two steady 30-second observation windows: `Window A` and `Window B`)
- **RSS Growth Ceiling**: <= 10% (`RELAY_SOAK_RSS_GROWTH_PCT`)
- **File Descriptor Growth Ceiling**: <= 8 (`RELAY_SOAK_FD_GROWTH`)
- **Thread/Task Growth Ceiling**: <= 4 (`RELAY_SOAK_TASK_GROWTH`)
- **p99 Latency Ceiling**: <= 250 ms (`RELAY_SOAK_P99_MS`), <= 100% growth (`RELAY_SOAK_P99_GROWTH_PCT`)
- **Errors Allowed**: 0

**Candidate Head Measurement Receipt (Run 40133268456)**:
- **Duration**: 60 seconds
- **Total Operations**: 9,258 ops (throughput: ~154.3 ops/sec)
- **Errors**: 0
- **RSS Window A**: 14,740 KB
- **RSS Window B**: 14,800 KB
- **Steady RSS Drift**: 0% (ceiling <= 10%)
- **FD Window A / B**: 13 / 13 (drift: 0, ceiling <= 8)
- **Tasks Window A / B**: 5 / 5 (drift: 0, ceiling <= 4)
- **p99 Latency Window A / B**: 0 ms / 0 ms (growth: 0%, ceiling <= 250 ms)
- **Provenance Artifact**: `restart-failover-soak-evidence-40133268456-1` (Artifact ID: `12575669906`, size: 237,110 bytes)
- **Receipt Status**: `passed`

## Related Ownership and Capability References

- **Cluster-Side Failover Owner**:
  - Rust test suite: [`crates/relay/tests/it/e2e/raft_failover.rs`](../../crates/relay/tests/it/e2e/raft_failover.rs)
  - Cloud GKE workflow: [`.github/workflows/gke-test.yml`](../../.github/workflows/gke-test.yml)
  - Status: `PENDING` (The live multi-voter GKE failover window is managed and executed in cloud GKE CI, not on GitHub runner or locally).

- **Related Capability and Contract Claims**:
  - Issue [#18](https://github.com/faberline/relay/issues/18) (`raft-hard-state-restart-safety` / multi-voter restart recovery in `raft_persistence.rs`)
  - Issue [#17](https://github.com/faberline/relay/issues/17) (`backup-restore-consistent-snapshot` / snapshot recovery under concurrent load in `backup.rs`)
  - Issue [#15](https://github.com/faberline/relay/issues/15) (`vat-meter-throughput-gate` / competitor driver benchmark and performance gate wiring in `measured_performance.rs`)
