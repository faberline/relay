# Operations

How to run, configure, and test relay.

| Document | What it covers |
|----------|----------------|
| [Root README](../../README.md) | Build, the one `relay` binary and its verbs, and the gate commands. |
| [ha.md](../ha.md) | Auto-mode raft HA, snapshots and the applied floor, the operator CR, backup and restore, and peer TLS. |
| [install.md](install.md) | Install from the image or from source, the three-replica quickstart, and a first publish and consume. |
| [runbook.md](runbook.md) | Operator runbook: node down, leader failover, disk full, backup, restore, drain and upgrade. |
| [upgrade.md](upgrade.md) | Version compatibility policy and the rolling-upgrade order. |
| [worker-protocol.md](../worker-protocol.md) | The HTTP/2 + OpenAPI contract that workers in any language use. |
| [perf-gate.md](../perf-gate.md) | The competitor perf gate and how to run it. |
| [restart-failover-soak.md](restart-failover-soak.md) | The restart, persistence, backup and bounded soak gate (`scripts/verify-restart-failover-soak.sh`) and its last recorded result. |
| [container-kustomize-verification.md](container-kustomize-verification.md) | The offline gate for the container and Kustomize artifacts and its last result. |

## Tests

```bash
cargo test -p relay --lib
cargo test -p relay --test it
cargo test -p relay --test raft_config
```

All integration tests are one binary, `it`. A test is named
`<file>::<test>`. Run one file with a module filter, for example
`cargo test -p relay --test it auth::`.
