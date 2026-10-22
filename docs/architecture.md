# Architecture

relay is a Cargo virtual workspace. Each bounded context is one crate under
`crates/`, and the `relay` assembly crate wires them together. This page says
how the source is cut and what may depend on what. The machine-checked form is
[`ddd.toml`](../ddd.toml); the decision behind it is in
[ADR 0003](adr/0003-multi-crate-workspace.md).

Everything that is not specific to relay — the HTTP service kit, bearer auth,
the raft runtime, peer TLS, service lifecycle, backups, the Kubernetes
operator kit, the standard CLI commands — comes from
[faberline/core](https://github.com/faberline/core) crates pinned by git tag.

## Contexts

| Context | Crate | Layer | Depends on | What it owns |
|---------|-------|-------|------------|--------------|
| `storage` | `relay-storage` | infrastructure | — | the durable, segmented `Log` per `(subject, shard)` |
| `broker` | `relay-broker` | application | storage | `Relay` (the engine facade), `WorkQueue`, and sharding |
| `replication` | `relay-replication` | infrastructure | broker | `RelayStateMachine` on raft-runtime, the lease reconciler, and backup shipping |
| `access` | `relay-access` | application | — | bearer auth over `core/service-auth` |
| `queue` | `relay-queue` | interfaces | access, broker, replication | the HTTP/h2c router, streaming consume, OpenAPI document, wire types, and metrics |
| `operator` | `relay-operator` | infrastructure | — | the `Relay` custom resource, the renderer, and the reconcile loop |

The shared kernel, `relay-shared-kernel`, holds the message, lease and offset
types and the broker config. Every context may use it.

Dependencies point one way: `storage ← broker ← replication ← queue`.
`access` is a leaf that `queue` uses. `operator` is an independent leaf.

## Assembly

The `relay` crate may use every context. Its library re-exports each context
by name (`relay::broker`, `relay::queue`, …) and keeps the flat re-exports.

| Path | What it does |
|------|--------------|
| `crates/relay/src/tls.rs`, `crates/relay/src/tls/` | installs the rustls provider and adapts the shared peer-mTLS material |
| `crates/relay/src/perf_gate.rs` | the competitor ratchet rule used by the perf gate tests |
| `crates/relay/src/bin/relay/` | the one `relay` binary: `cli.rs` owns the clap surface and dispatch, one module per verb |

## Source layout

```text
crates/
  relay-shared-kernel/   config, types
  relay-storage/         log
  relay-broker/          engine, shard, workqueue
  relay-replication/     raft, reconciler, backup (feature `backup`)
  relay-access/          auth
  relay-queue/           consume, metrics, openapi, server, server_config, wire
  relay-operator/        crd, reconcile, render
  relay/
    src/lib.rs           the context re-exports and the flat public API
    src/tls.rs + tls/    peer
    src/perf_gate.rs
    src/bin/relay/       main.rs, cli.rs, output.rs, one file per verb
    tests/it/main.rs     the one integration binary; each file is one module
    tests/raft_config.rs its own binary: it changes process-global env vars
    benches/, examples/
```

A module is a file `src/<name>.rs` with an optional folder `src/<name>/`.
`clippy::mod_module_files` denies `mod.rs`.
