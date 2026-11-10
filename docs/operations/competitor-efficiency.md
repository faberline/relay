# Competitor semantics and efficiency verification

Evidence record for issue #50 (competitor semantics and efficiency, local
scope). One command runs the local in-process competitor-performance test
modules and the release-mode measured durable lifecycle gate. Run it with:

```bash
bash scripts/verify-competitor-efficiency.sh
```

## Scope

- Local, in-process only. The gate starts no relay process, no cluster and
  makes no network call.
- The real-peer comparison (RabbitMQ, NATS, Redis, Dragonfly) stays advisory.
  It, the GKE run and any cloud run are a manual pre-release check; see
  [perf-gate.md](../perf-gate.md). This record does not cover them.
- Decision: 2026-11-10 comment on #50 (aw-service-desk, on behalf of the
  user). The live, GKE and peer work is not in this issue.

## Commit

`3b5bfa472b731f87252ee7e71be3c55bb99cfb0d` (`git rev-parse HEAD` at the run).
The run used the working tree on top of that commit, with the wrapper, its
tests and this record added but not yet committed.

## Environment

- OS: Linux 6.18.44-fc-v338
- `rustc --version`: rustc 1.97.0 (2d8144b78 2026-07-07)
- `cargo --version`: cargo 1.97.0 (c980f4866 2026-06-30)
- Run duration: 5m 18s wall clock (318 s), including the release build.

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
`relay_perf_gate` marker or any `errors` value other than 0 fails the gate.

## Output

```
>> work_queue_throughput: 6 passed
>> perf_gate: 5 passed
>> measured_performance: 3 passed
>> measured: relay_perf_gate workload=relay-durable-publish-lease-ack-v1 ops=2000 samples=20/20 publish=194514.7msg/s lease_ack=195331.6msg/s p95=1149/722us errors=0
>> competitor-efficiency: passed
```

Test counts are non-zero for every module and all passed. The measured gate
line reports `errors=0`.

## Checks around the run

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass.
- `cargo test --locked -p relay --test it verify_competitor_efficiency_script::`:
  4 passed (`script_is_executable`, `zero_test_module_fails_the_gate`,
  `empty_module_list_fails_the_gate`, `module_with_tests_passes_without_measured_gate`).
- `cargo test --locked --workspace`: exit 0, no failed test.

## Cleanup

The `cleanup` trap removes the script's temp directory on success and failure.
The script starts no process. No `ce-verify.*` temp directory was left after
the run.

## Result

`passed`

## Classification

A shared or non-domain failure keeps the issue open until it is fixed and the
gate is rerun. A failure that only an app domain owns becomes a new bounded
issue, and the gate records it as a tracked skip. No such failure occurred in
this run, and no duplicate issue was opened.
