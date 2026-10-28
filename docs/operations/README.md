# Operations

How to run, configure, and test relay.

| Document | What it covers |
|----------|----------------|
| [Root README](../../README.md) | Build, the one `relay` binary and its verbs, and the gate commands. |
| [ha.md](../ha.md) | Auto-mode raft HA, snapshots and the applied floor, the operator CR, backup and restore, and peer TLS. |
| [worker-protocol.md](../worker-protocol.md) | The HTTP/2 + OpenAPI contract that workers in any language use. |
| [perf-gate.md](../perf-gate.md) | The competitor perf gate and how to run it. |
| [restart-failover-soak.md](restart-failover-soak.md) | The restart, failover, and soak verification axis gate and evidence record. |

## Tests

```bash
cargo test -p relay --lib
cargo test -p relay --test it
cargo test -p relay --test raft_config
```

All integration tests are one binary, `it`. A test is named
`<file>::<test>`. Run one file with a module filter, for example
`cargo test -p relay --test it auth::`.
