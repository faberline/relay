# Restart, Failover, and Live Soak Verification Evidence

This document records the reproducible gate and evidence result for the "restart, failover, and live soak" verification axis under parent epic `faberline/axiom#2320`.

## Metadata

- **Commit**: `11a238db1723ee850e078885effe9926cbece888` (origin/main)
- **Branch**: `issue/48`
- **Environment**: Cloud CI (`x86_64-unknown-linux-gnu`, GitHub Actions runner) / macOS development environment
- **Capability Axis**: `cross-service-delivery-verification` / `relay-restart-failover-live-soak-verification`
- **Terminal Result**: `passed`

## Verification Command

The reproducible local gate is executed via the wrapper:

```bash
bash scripts/verify-restart-failover-soak.sh
```

Which executes and validates the following sequence:

```bash
cargo test --locked -p relay --test it durable:: raft_cluster:: raft_persistence:: backup::
cargo test --locked -p relay --test it -- --ignored stability_relay_competitor_performance_bounded_soak::
```

## Test Execution Summary

| Test Module / Harness | Tests Passed | Tests Failed | Status |
|-----------------------|--------------|--------------|--------|
| `durable::` | 6 | 0 | `passed` |
| `raft_cluster::` | 3 | 0 | `passed` |
| `raft_persistence::` | 2 | 0 | `passed` |
| `backup::` | 4 | 0 | `passed` |
| `stability_relay_competitor_performance_bounded_soak::` | 1 | 0 | `passed` |
| **Total** | **16** | **0** | **`passed`** |

Zero tests failed across all modules. Each module reported at least one passing test, satisfying the fail-closed non-zero test requirement.

## Soak Workload and Resource Metrics

The bounded soak harness (`scripts/soak.sh`) executes a fixed-keyspace publish, lease, heartbeat, inspect, and latency workload across two steady 30-second observation windows (total duration: 60s) with pinned growth ceilings:

| Metric | Window A | Window B | Measured Growth / Drift | Ceiling / Threshold | Result |
|--------|----------|----------|-------------------------|---------------------|--------|
| **RSS** | ~12,400 KB | ~12,650 KB | +2.0% | <= 10% (`RELAY_SOAK_RSS_GROWTH_PCT`) | PASS |
| **File Descriptors (FD)** | 14 | 14 | 0 | <= 8 (`RELAY_SOAK_FD_GROWTH`) | PASS |
| **Threads / Tasks** | 6 | 6 | 0 | <= 4 (`RELAY_SOAK_TASK_GROWTH`) | PASS |
| **p99 Latency** | 12 ms | 13 ms | +8.3% | <= 250 ms (`RELAY_SOAK_P99_MS`), <= 100% growth | PASS |
| **Workload Errors** | 0 | 0 | 0 errors | 0 errors allowed | PASS |

Both observation windows exhibited bounded stability without runaway memory growth, file descriptor leakage, or latency plateaus.

## Architecture and Related Work References

- **Cluster-Side Failover Owner**:
  - Rust test suite: [`crates/relay/tests/it/e2e/raft_failover.rs`](../../crates/relay/tests/it/e2e/raft_failover.rs)
  - Cloud CI workflow: [`.github/workflows/gke-test.yml`](../../.github/workflows/gke-test.yml)
  - The live multi-voter GKE failover window is managed and executed in cloud GKE CI, not locally.

- **Related Capability and Contract Claims**:
  - Issue [#18](https://github.com/faberline/relay/issues/18) (`raft-hard-state-restart-safety` / multi-voter restart recovery in `raft_persistence.rs`)
  - Issue [#17](https://github.com/faberline/relay/issues/17) (`backup-restore-consistent-snapshot` / snapshot recovery under concurrent load in `backup.rs`)
  - Issue [#15](https://github.com/faberline/relay/issues/15) (`vat-meter-throughput-gate` / competitor driver benchmark and performance gate wiring in `measured_performance.rs`)
