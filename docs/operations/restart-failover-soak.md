# Restart, failover and soak verification

Evidence record for issue #48 (restart, failover, and live soak). One command
runs the local restart, persistence and backup test modules and the bounded
work-queue soak. Run it with:

```bash
bash scripts/verify-restart-failover-soak.sh
```

## Commit

`f46a32a596a19247ec095a06eafedaf3522c82ae` (`git rev-parse HEAD` at the run).
The run used the working tree on top of that commit, with the wrapper, its
tests and this record added but not yet committed.

## Environment

- OS: Linux 6.18.44-fc-v338
- `rustc --version`: rustc 1.97.0 (2d8144b78 2026-07-07)
- Local relay at `http://127.0.0.1:7000`, started by `scripts/soak.sh` with
  `RELAY_SOAK_AUTOSTART=1` and stopped when the script exits.
- Run duration: 4m 08s wall clock.

## Command

```bash
bash scripts/verify-restart-failover-soak.sh
```

The script runs these steps, each one a fresh `cargo test`:

- `cargo test --locked -p relay --test it -- durable::`
- `cargo test --locked -p relay --test it -- raft_cluster::`
- `cargo test --locked -p relay --test it -- raft_persistence::`
- `cargo test --locked -p relay --test it -- backup::`
- `RELAY_SOAK_AUTOSTART=1 bash scripts/soak.sh`

A module that runs 0 tests, a failed step, `total_ops` of 0, or any soak error
fails the gate.

## Output summary

Test counts (non-zero, all passed):

| Module | Passed |
|---|---|
| `durable` | 6 |
| `raft_cluster` | 3 |
| `raft_persistence` | 2 |
| `backup` | 4 |

Soak report (`scripts/soak.sh`, two 30 s steady windows):

| Field | Value |
|---|---|
| total_ops | 10404 |
| errors | 0 |
| rss_window_a | 14724 KB |
| rss_window_b | 14736 KB |
| steady_drift | 0% (window A to window B) |
| fd_window | 11 to 11 |
| tasks_window | 5 to 5 |
| p99_window | 1 ms to 1 ms |

Final lines of the run: `>> Relay bounded work-queue soak PASS`, then
`>> restart-failover-soak: passed`.

Cleanup: no relay process and no `rfs-soak.*` or `relay-soak*` temp
directory were left after the run.

## Result

`passed`

## Cluster-side owner

The GKE failover suite is not run by this record. Its owners are
`crates/relay/tests/it/e2e/raft_failover.rs` and the CI job `e2e: raft_failover`
in `.github/workflows/ci.yml`.

## Related issues

- #18 owns hard-state restart safety.
- #17 and #15 own the backup/restore snapshot and the meter gate.

No duplicate issues were opened.
