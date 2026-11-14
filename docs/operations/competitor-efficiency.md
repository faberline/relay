# Competitor semantics and efficiency verification

Evidence record for issue #50 (local competitor semantics and efficiency,
under parent epic faberline/axiom#2318). One command runs the local
in-process competitor-performance test modules and the release-mode measured
durable lifecycle gate. Run it with:

```bash
bash scripts/verify-competitor-efficiency.sh
```

## Scope

This record covers the local, in-process part only. It uses no cloud
resources, no GKE cluster, no external process and no network.

The real-peer comparison (RabbitMQ, NATS, Redis, Dragonfly) and the live or
GKE runs are not part of this gate. They stay advisory and are a manual
pre-release check; see [../perf-gate.md](../perf-gate.md).

## Commit

`79042bce8eb4a626113857372c15af98a9c6ed28` (`git rev-parse HEAD` at the run).
The wrapper, its offline tests and the doc links were committed at that
commit. This record was added after the run and is not part of the measured
tree.

## Environment

- OS: Linux 6.18.44-fc-v445
- `rustc --version`: rustc 1.97.0 (2d8144b78 2026-07-07)
- Run duration: 2m 00s wall clock (release build of the measured gate
  included).

## Command

```bash
bash scripts/verify-competitor-efficiency.sh
```

The script runs these steps, each one a fresh `cargo test`:

- `cargo test --locked -p relay --test it -- work_queue_throughput::`
- `cargo test --locked -p relay --test it -- perf_gate::`
- `cargo test --locked -p relay --test it -- measured_performance::`
- `cargo test -q --release --locked -p relay --test it measured_performance::measured_durable_lifecycle_gate -- --exact --ignored --nocapture`

A module that runs 0 tests, an empty module list, a failed step, a missing
measured marker or `errors` other than 0 fails the gate.

## Output summary

Test counts (non-zero, all passed):

| Module | Passed |
|---|---|
| `work_queue_throughput` | 6 |
| `perf_gate` | 5 |
| `measured_performance` | 3 |

Measured gate (`measured_durable_lifecycle_gate`, release build):

```text
relay_perf_gate workload=relay-durable-publish-lease-ack-v1 ops=2000 samples=20/20 publish=4800.1msg/s lease_ack=184825.8msg/s p95=24805/852us errors=0
```

Final lines of the run:

```text
>> work_queue_throughput: 6 passed
>> perf_gate: 5 passed
>> measured_performance: 3 passed
>> measured: relay_perf_gate workload=relay-durable-publish-lease-ack-v1 ops=2000 samples=20/20 publish=4800.1msg/s lease_ack=184825.8msg/s p95=24805/852us errors=0
>> competitor-efficiency: passed
```

Cleanup: the wrapper's temp directory is removed by its `cleanup` trap, and
it starts no process.

## Classification

A shared or non-domain failure keeps the issue open until it is fixed and
the affected modules are rerun. A failure that only an app domain owns
becomes a new bounded issue, linked from this record, and is recorded as a
tracked skip.

## Result

`passed`
