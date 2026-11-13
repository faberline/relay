# Competitor semantics and efficiency verification

Evidence record for issue #50 (local competitor semantics and efficiency,
unit-test gated). One command runs the local in-process competitor-performance
test modules and the release-mode measured durable lifecycle gate. Run it with:

```bash
bash scripts/verify-competitor-efficiency.sh
```

## Scope

This record covers local, in-process verification only. The live peer
comparison against RabbitMQ, NATS, Redis and Dragonfly, and any GKE or cloud
run, are not part of this command. They stay advisory and are a manual
pre-release check (see `docs/perf-gate.md`). No external service, paid
infrastructure or network access is used by this run.

## Commit

`10c0ab481aa859e44faae722b26038ffdc112c93` (`git rev-parse HEAD` at the run).
The run used the working tree on top of that commit, with the wrapper, its
tests, the README row and the perf-gate bullet added but not yet committed.

## Environment

- OS: Linux 6.18.44-fc-v445
- `rustc --version`: rustc 1.97.0 (2d8144b78 2026-07-07)
- Run duration: 5m 39s wall clock (includes the release build of the measured gate).

## Command

```bash
bash scripts/verify-competitor-efficiency.sh
```

The script runs these steps:

- `cargo test --locked -p relay --test it -- work_queue_throughput::`
- `cargo test --locked -p relay --test it -- perf_gate::`
- `cargo test --locked -p relay --test it -- measured_performance::`
- `cargo test -q --release --locked -p relay --test it measured_performance::measured_durable_lifecycle_gate -- --exact --ignored --nocapture`

A module that runs 0 tests, an empty module list, a failed step, or a measured
marker with `errors != 0` fails the gate.

## Output summary

Test counts (non-zero, all passed):

| Module | Passed |
|---|---|
| `work_queue_throughput` | 6 |
| `perf_gate` | 5 |
| `measured_performance` | 3 |

Measured gate (release build, `measured_durable_lifecycle_gate`):

```
relay_perf_gate workload=relay-durable-publish-lease-ack-v1 ops=2000 samples=20/20 publish=155086.8msg/s lease_ack=201714.6msg/s p95=1767/762us errors=0
```

Final lines of the run:

```
>> competitor-efficiency: passed
```

Cleanup: the script removes its temp directory through its `cleanup` trap and
starts no process; no relay process or cluster is used.

## Classification

A shared or non-domain failure keeps issue #50 open until it is repaired and
rerun. A failure that only concerns app-domain behavior becomes a new bounded
issue, and this record then names it as a tracked skip. This run had no failure
to classify.

## Result

`passed`
