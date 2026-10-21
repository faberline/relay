# Architecture

relay is one Cargo crate. Each bounded context is one top-level module under
`src/`, and the `relay` binary wires them together. This page says how the
source is cut and what may depend on what. The machine-checked form is
[`ddd.toml`](../ddd.toml); the decision behind it is in
[ADR 0001](adr/0001-single-crate-layered-modules.md).

Everything that is not specific to relay — the HTTP service kit, bearer auth,
the raft runtime, peer TLS, service lifecycle, backups, the Kubernetes
operator kit, the standard CLI commands — comes from
[faberline/core](https://github.com/faberline/core) crates pinned by git tag.

## Contexts

| Context | Module | Layer | Depends on | What it owns |
|---------|--------|-------|------------|--------------|
| `broker` | `src/broker` | application | — | `Relay` (the engine facade), the durable `Log`, `WorkQueue`, sharding, config, and the message, lease and offset types |
| `replication` | `src/cluster` | infrastructure | broker | `RelayStateMachine` on raft-runtime, the lease reconciler, and backup shipping |
| `transport` | `src/transport` | interfaces | broker, replication | the HTTP/h2c router, streaming consume, auth, OpenAPI document, wire types, and metrics |
| `operator` | `src/operator` | infrastructure | — | the `Relay` custom resource, the renderer, and the reconcile loop (feature `operator`) |

Dependencies point one way: `broker ← replication ← transport`. `operator` is
an independent leaf.

## Assembly

These modules may use every context:

| Module | What it does |
|--------|--------------|
| `src/tls.rs`, `src/tls/` | installs the rustls provider and adapts the shared peer-mTLS material |
| `src/perf_gate.rs` | the competitor ratchet rule used by the perf gate tests |
| `src/bin/relay/` | the one `relay` binary: `cli.rs` owns the clap surface and dispatch, one module per verb |

## Source layout

```text
src/
  lib.rs                 module wiring and the public re-exports
  broker.rs + broker/    config, engine, log, shard, types, workqueue
  cluster.rs + cluster/  raft, reconciler, backup
  transport.rs + transport/
                         auth, consume, metrics, openapi, server, server_config, wire
  operator.rs + operator/
                         crd, reconcile, render
  tls.rs + tls/          peer
  perf_gate.rs
  bin/relay/             main.rs, cli.rs, output.rs, one file per verb
tests/
  it/main.rs             the one integration binary; each file is one module
  raft_config.rs         its own binary: it changes process-global env vars
```

A module is a file `src/<name>.rs` with an optional folder `src/<name>/`.
`clippy::mod_module_files` denies `mod.rs`.
