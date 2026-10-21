// HANDWRITE-BEGIN gap="missing-generator:logic:1eefd229" tracker="pending-tracker" reason="Single relay CLI bin (clap): bare relay (no subcommand) runs the h2c server with ServeArgs flags falling back to RELAY_BIND/RELAY_DATA_DIR env (the relay_server.rs behavior verbatim); Command::Llm/Upgrade/Issue dispatch to cli_std::{llm,upgrade,issue} with relay's ToolInfo; mirrors projects/keep/src/bin/keep.rs."
//! relay — cloud-native work-queue broker (HTTP/2 + OpenAPI).
//!
//! Bare `relay` (no subcommand) runs the server — the former `relay-server`
//! entrypoint verbatim (env-driven; flags override). The standard agent-facing
//! commands — `relay llm`, `relay upgrade`, `relay issue` (all offline-safe,
//! network paths behind the `self-update`/`issue` features via the shared
//! `cli-std` lib) — sit alongside it per the CONTRIBUTING.md CLI convention.
//! Agents start at `relay llm`.
//!
//! Each verb lives in its own module. `cli` owns the clap surface, the
//! dispatch, and the CLI parse tests.

use anyhow::Result;

mod backup;
mod cli;
mod dockerfile;
mod issue;
mod k8s;
mod llm;
mod output;
mod serve;
mod spec;
mod upgrade;

#[tokio::main]
async fn main() -> Result<()> {
    cli::run().await
}
// HANDWRITE-END
