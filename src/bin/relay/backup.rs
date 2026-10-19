//! `relay backup`: snapshot a running node over HTTP and ship it to a
//! backup destination (feature `backup`).

use anyhow::Result;

/// `relay backup` flags (#1209): pulls a snapshot over HTTP from a running
/// node and ships it to a destination via `core/service-backup` (lumen #808).
#[derive(clap::Args, Debug)]
pub(crate) struct BackupArgs {
    /// Base URL of a running relay node, e.g.
    /// `http://<name>.<namespace>.svc.cluster.local:7000` (what the operator's
    /// backup CronJob passes) or `http://localhost:7000` for ad hoc use.
    #[arg(long)]
    pub(crate) url: String,
    /// Destination URI: `file:///path`, `s3://bucket/prefix`, or schema-only
    /// `gs://bucket/prefix` (parses, but the runner supports `file://` and
    /// `s3://` sinks today).
    #[arg(long)]
    pub(crate) dest: String,
    /// Bearer token for `/admin/backup` (needs `admin` on `*`). Falls back to
    /// `RELAY_BACKUP_TOKEN`; omit entirely when the node runs `--auth off`.
    #[arg(long, env = "RELAY_BACKUP_TOKEN")]
    pub(crate) token: Option<String>,
    /// Drop backup objects older than this many seconds after a successful
    /// put. Omit to keep everything.
    #[arg(long)]
    pub(crate) retention_secs: Option<u64>,
}

/// `relay backup` (#1209): fetch `{url}/admin/backup` and ship the bytes to
/// `--dest` via `core/service-backup`, printing the resulting
/// `BackupRunResult` as JSON. This is what the operator's optional backup
/// CronJob (`spec.backup`) invokes on a schedule; it works equally ad hoc.
#[cfg(feature = "backup")]
pub(crate) async fn run(args: BackupArgs) -> Result<()> {
    let dest = service_backup::BackupDestination::from_uri(&args.dest)?;
    let retention = match args.retention_secs {
        Some(secs) => service_backup::RetentionPolicy::max_age_seconds(secs),
        None => service_backup::RetentionPolicy::default(),
    };
    let result =
        relay::cluster::backup::run_backup(&args.url, args.token.as_deref(), &dest, &retention)
            .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("next: done");
    Ok(())
}

#[cfg(not(feature = "backup"))]
pub(crate) async fn run(_args: BackupArgs) -> Result<()> {
    anyhow::bail!(
        "this relay build was compiled without backup support; rebuild with \
         `--features backup` (the published image includes it)"
    )
}
