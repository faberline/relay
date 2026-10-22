# ADR 0003 — Virtual workspace, one crate per context

## Status

Accepted. Supersedes [ADR 0001](0001-single-crate-layered-modules.md).

## Context

ADR 0001 kept relay as one crate, with one top-level module per context, and
left the crate split for a later change. The faberline SaaS services (lumen,
tape, defer, relay and keep) now share one crate skeleton
(faberline/aw `docs/standards/saas-service.md`). Each one is a virtual
workspace with a shared kernel, an assembly crate, and one crate per context.
Every one of them stores data, so each has a `-storage` crate.

## Decision

- The root `Cargo.toml` is a virtual workspace: `[workspace]`,
  `[workspace.package]`, `[workspace.dependencies]`, the lints and the
  profiles. Members live under `crates/<package>`. The release version is the
  first column-0 `version = "x.y.z"` line of the root manifest.
- One crate per context, named `relay-<context>`:
  - `relay-shared-kernel`: the data model (`types`) and the broker config.
  - `relay-storage`: the durable, segmented log.
  - `relay-broker`: the engine facade, shard routing and the work queue.
  - `relay-replication`: the raft state machine, the lease reconciler and
    backup shipping.
  - `relay-access`: bearer auth over `core/service-auth`.
  - `relay-queue`: the HTTP/h2c router, wire types, streaming consume,
    OpenAPI and metrics.
  - `relay-operator`: the `Relay` custom resource, the renderer and the
    reconcile loop.
- Dependencies point one way: `shared-kernel ← storage ← broker ←
  replication ← queue`. `access` is a leaf that `queue` uses. `operator` is
  an independent leaf.
- `relay` is the assembly crate. Its library re-exports each context by name
  (`relay::broker`, `relay::queue`, …) and keeps the flat re-exports
  (`relay::Relay`, `relay::router`, …). It holds the rustls provider install,
  the perf-gate rule, the `relay` binary and the integration tests.
- Feature names do not change. `operator` enables the optional
  `relay-operator` dependency, so the default build does not link kube.
  `backup` also enables `relay-replication/backup`.
- `ddd.toml` is enforced.

## Consequences

- Each context boundary is a crate boundary, so the compiler enforces it.
- `cargo test -p relay` still runs the integration tests. CI runs
  `cargo test --workspace` so the unit tests in every crate run too.
- Paths in old tech-design records (`src/broker`, `src/cluster`,
  `src/transport`, `src/operator`) describe the code before this change.
