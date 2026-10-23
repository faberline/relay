# ADR 0001 — One crate, one top-level module per context

## Status

Superseded by [ADR 0003](0003-multi-crate-workspace.md).

## Context

The faberline standards ask every repository to name its bounded contexts and
their layers in `ddd.toml`, so `aw standards rust-arch` can check the
dependency direction. relay is small: one library, one binary, and an optional
operator. Its code was already grouped by layer into `broker`, `cluster`,
`transport`, `operator` and `tls`.

## Decision

- relay stays one crate on faberline/core. A crate split is a separate change.
- Each context is one top-level module: `broker`, `replication`
  (`src/cluster`), `transport` and `operator`. `tls`, `perf_gate` and the
  binary are assembly.
- Dependencies point one way: `broker ← replication ← transport`. `operator`
  is a leaf.
- A module is `src/<name>.rs` plus an optional folder. `mod.rs` is denied by
  `clippy::mod_module_files`.
- The binary's `main.rs` only declares modules and calls `cli::run`.
- `ddd.toml` starts with `[migration] enforce = false`, so the checker only
  reports. The single-member `[workspace]` stays until the crate split.

## Consequences

- The context boundaries are visible in the source tree and checked by
  `aw standards rust-arch`.
- Whole modules carry one layer each. A finer cut or a crate split is a later
  change.
