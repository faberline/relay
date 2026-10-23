//! Bare `relay` (no subcommand): run the broker server — the former
//! `relay-server` entrypoint, env-driven with flag overrides.

use std::time::Duration;

use anyhow::{Context, Result};
use clap::ValueEnum;

use relay::queue::server::AppState;
use relay::queue::server_config::RelayServerConfig;
use relay::spawn_reconciler;

/// Server flags (the bare `relay` path) — the `relay-server` env knobs
/// surfaced as flags with env fallback.
#[derive(clap::Args, Debug)]
pub(crate) struct ServeArgs {
    /// h2c listen address for this shard.
    #[arg(long, env = "RELAY_BIND", default_value = "0.0.0.0:7000")]
    pub(crate) bind: String,
    /// Durable log directory (defaults to the core config's data dir).
    #[arg(long, env = "RELAY_DATA_DIR")]
    pub(crate) data_dir: Option<String>,
    /// Graceful-drain window (seconds) held after SIGTERM before the listener
    /// closes, while `/readyz` reports 503 so k8s stops routing.
    #[arg(long, env = "RELAY_GRACE_SECS", default_value_t = 10)]
    pub(crate) grace_secs: u64,
    /// Log output format. Kubernetes uses `json` for the shared
    /// `axiom.service.log.v1` collector contract; local development defaults
    /// to the human-readable formatter.
    #[arg(long, env = "RELAY_LOG_FORMAT", value_enum, default_value_t = LogFormat::Pretty)]
    pub(crate) log_format: LogFormat,
    /// OTLP/gRPC trace collector endpoint. Requires a build with `--features otel`.
    #[arg(long, env = "RELAY_OTLP_ENDPOINT")]
    pub(crate) otlp_endpoint: Option<String>,
    /// Request-auth mode for the /v1 data plane: `off` (tokenless dev,
    /// the default) or `required` (bearer tokens from the registry file).
    /// Probes stay tokenless either way.
    #[arg(long, env = "RELAY_AUTH", default_value = "off")]
    pub(crate) auth: String,
    /// Bearer-token registry file (JSON `{token: {subject, roles}}`),
    /// mounted from a Secret in production. Required (and validated at
    /// startup) when `--auth required`.
    #[arg(long, env = "RELAY_TOKEN_REGISTRY_FILE")]
    pub(crate) token_registry_file: Option<String>,
    /// Headless-Service name for raft peer DNS in replica/HA mode
    /// (`relay-<ordinal>.<peer-service>:<serve port>`). Only read when the
    /// standard downward-API env says `REPLICAS_PER_SHARD > 1`.
    #[arg(long, env = "RELAY_PEER_SERVICE", default_value = "relay")]
    pub(crate) peer_service: String,
    /// Dedicated authenticated Raft listener when RELAY_PEER_MTLS=on.
    #[arg(long, env = "RELAY_RAFT_PORT", default_value_t = 7001)]
    pub(crate) raft_port: u16,
    /// Data-plane request body size limit (bytes). Requests with
    /// `Content-Length` exceeding this are rejected with 413; streamed bodies
    /// are bounded mid-read. Defaults to 8 MiB (`RELAY_BODY_LIMIT_BYTES`).
    #[arg(long, env = "RELAY_BODY_LIMIT_BYTES", default_value_t = 8 * 1024 * 1024)]
    pub(crate) body_limit_bytes: usize,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum LogFormat {
    Pretty,
    Json,
}

/// Run the relay server (the default, no-subcommand path) — the former
/// `relay-server` entrypoint: load config, spawn the lease reconciler, serve
/// the app through the shared service shell (#1205): HTTP/1.1 + h2c on one
/// port with a SIGTERM-aware graceful drain (`--grace-secs`).
pub(crate) async fn run(args: ServeArgs) -> Result<()> {
    let log_format = match args.log_format {
        LogFormat::Pretty => service_http::LogFormat::Pretty,
        LogFormat::Json => service_http::LogFormat::Json,
    };
    let tracing_config = service_http::HttpConfig::new(
        "127.0.0.1",
        0,
        "info",
        log_format,
        args.grace_secs,
        0,
        args.otlp_endpoint.clone(),
    );
    let tracing_identity = service_http::ServiceIdentity::new("relay", env!("CARGO_PKG_VERSION"))?;
    service_http::init_tracing_with_identity(&tracing_config, &tracing_identity)?;

    // Resolve the bearer-auth contract (#1206) BEFORE anything serves: with
    // --auth required a missing/unparseable/empty registry file is a startup
    // error (nonzero exit), never a per-request 401.
    let auth = relay::access::auth::AuthConfig::resolve(
        &args.auth,
        args.token_registry_file.as_deref(),
        std::env::var(relay::access::auth::LEGACY_TOKENS_ENV)
            .ok()
            .as_deref(),
    )?;
    tracing::info!(
        required = auth.required,
        "request auth resolved (RELAY_AUTH; probes stay tokenless)"
    );
    let admission = service_http::AdmissionConfig::from_env("RELAY")?.controller(
        "relay.read",
        "relay.write",
        "relay.admin",
    );
    if admission.is_some() {
        tracing::info!(
            "request admission enabled (RELAY_ADMISSION_*; probes and peer routes stay exempt)"
        );
    }

    let mut core = relay::RelayCoreConfig::default();
    if let Some(data_dir) = args.data_dir {
        core.data_dir = data_dir;
    }
    let config = RelayServerConfig {
        bind: args.bind,
        core,
        ..RelayServerConfig::default()
    };
    let bind = config.bind.clone();
    let reconcile_interval = Duration::from_millis(config.reconcile_interval_ms);

    let mut state = AppState::with_auth(config, auth, args.body_limit_bytes);
    if let Some(path) = args.token_registry_file.as_deref() {
        std::mem::drop(service_auth::spawn_registry_file_watcher(
            state.verifier(),
            path,
        ));
    }
    // Auto-mode HA (#544): the standard downward-API quartet flips replica
    // mode (REPLICAS_PER_SHARD > 1) — no relay-specific flags. Topology comes
    // from raft-runtime (never re-derive the ordinal math locally); the raft
    // group replicates publishes into this process's engine and its peer
    // router rides the serve port OUTSIDE the bearer-auth data plane (cluster
    // traffic, tokenless like probes; mTLS is a later slice). Held for the
    // process lifetime — dropping it would abort the tick/pump tasks.
    let mut peer_transport = None;
    let raft = if raft_runtime::cluster::replica_mode() {
        let transport = relay::tls::peer::PeerTlsConfig::from_env()?
            .map(|config| config.peer_transport())
            .transpose()?;
        let (peer_port, peer_scheme) = match transport.as_ref() {
            Some(_) => (args.raft_port, "https"),
            None => (
                bind.rsplit(':')
                    .next()
                    .and_then(|port| port.parse::<u16>().ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!("cannot derive the raft peer port from --bind {bind}")
                    })?,
                "http",
            ),
        };
        let topo = raft_runtime::ClusterTopology::from_env_with_scheme(
            "relay",
            &args.peer_service,
            peer_port,
            "RELAY_PEERS",
            peer_scheme,
        )?;
        let data_dir = state.config().core.data_dir.clone();
        anyhow::ensure!(
            !data_dir.is_empty(),
            "replica/HA mode requires a durable --data-dir (RELAY_DATA_DIR)"
        );
        let raft = std::sync::Arc::new(match transport.clone() {
            Some(transport) => relay::RelayRaft::from_topology_with_peer_transport(
                state.relay_handle(),
                std::path::Path::new(&data_dir),
                &topo,
                relay::RelayRaft::host_config(relay::replication::raft::SNAPSHOT_EVERY),
                transport,
            )?,
            None => relay::RelayRaft::from_topology(
                state.relay_handle(),
                std::path::Path::new(&data_dir),
                &topo,
                relay::RelayRaft::host_config(relay::replication::raft::SNAPSHOT_EVERY),
            )?,
        });
        state.set_raft(std::sync::Arc::clone(&raft));
        peer_transport = transport;
        tracing::info!(
            node_id = topo.node_id,
            replicas = topo.replicas_per_shard,
            voters = topo.membership.voters.len(),
            peer_scheme,
            peer_port,
            "raft: replica/HA mode — all queue mutations replicate"
        );
        Some(raft)
    } else {
        None
    };

    // Held for the process lifetime. In replica mode expiry/reclaim is itself
    // a committed mutation; single-node mode retains the direct engine sweep.
    let _reconciler = match &raft {
        Some(raft) => {
            relay::spawn_replicated_reconciler(std::sync::Arc::clone(raft), reconcile_interval)
        }
        None => spawn_reconciler(state.relay_handle(), reconcile_interval),
    };

    let mut app = relay::queue::server::router_with_admission(state.clone(), admission);
    if peer_transport.is_none() {
        if let Some(raft) = &raft {
            app = app.merge(raft.router());
        }
    }
    let peer_router = match (&raft, &peer_transport) {
        (Some(raft), Some(_)) => Some(raft.router()),
        _ => None,
    };
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(
        addr = %listener.local_addr()?,
        "relay listening (HTTP/1.1 + HTTP/2 cleartext)"
    );

    let peer_server = match (peer_transport, peer_router) {
        (Some(transport), Some(router)) => {
            let peer_bind = peer_bind_address(&bind, args.raft_port)?;
            let peer_listener = tokio::net::TcpListener::bind(&peer_bind).await?;
            let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
            let serve = tokio::spawn(async move {
                transport
                    .serve(peer_listener, router, async move {
                        let _ = shutdown_rx.await;
                    })
                    .await
            });
            Some((shutdown, serve))
        }
        (None, None) => None,
        _ => unreachable!("peer transport and router are configured together"),
    };

    // Serve HTTP/1.1 + h2c on one port and drain on SIGTERM through the shared
    // service shell: `start_drain` flips `/readyz` to 503 for the grace window
    // before the listener closes.
    let grace = Duration::from_secs(args.grace_secs);
    service_http::serve(
        listener,
        app,
        service_http::shutdown_with_drain(move || state.start_drain(), grace),
    )
    .await;
    if let Some((shutdown, serve)) = peer_server {
        let _ = shutdown.send(());
        serve.await.context("relay peer listener task")??;
    }
    Ok(())
}

fn peer_bind_address(bind: &str, raft_port: u16) -> Result<String> {
    let (host, _) = bind
        .rsplit_once(':')
        .with_context(|| format!("cannot derive peer bind address from {bind}"))?;
    anyhow::ensure!(!host.is_empty(), "--bind must include a host");
    Ok(format!("{host}:{raft_port}"))
}
