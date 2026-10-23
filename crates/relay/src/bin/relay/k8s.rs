//! `relay k8s`: render the CRD, operator control plane, and Relay instance
//! CRs, or run the operator (feature `operator`).

use std::path::PathBuf;

use anyhow::Result;
use clap::{Subcommand, ValueEnum};

use crate::output::write_or_print;

/// `relay k8s <crd|operator|instance>` — cluster artifacts split by lifecycle
/// layer.
#[derive(clap::Args, Debug)]
pub(crate) struct K8sArgs {
    #[command(subcommand)]
    pub(crate) cmd: K8sCmd,
}

#[derive(Subcommand, Debug)]
pub(crate) enum K8sCmd {
    /// Cluster-scoped API layer: render the Relay CRD.
    Crd(K8sCrdArgs),
    /// Operator control-plane layer: render assets or run the controller.
    Operator(K8sOperatorArgs),
    /// App-namespace declaration: render a Relay custom resource.
    Instance(K8sInstanceArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct K8sCrdArgs {
    #[command(subcommand)]
    pub(crate) cmd: K8sCrdCmd,
}

#[derive(Subcommand, Debug)]
pub(crate) enum K8sCrdCmd {
    /// Render the Relay CustomResourceDefinition YAML.
    Render(K8sFileOutputArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct K8sOperatorArgs {
    #[command(subcommand)]
    pub(crate) cmd: Option<K8sOperatorCmd>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum K8sOperatorCmd {
    /// Container entrypoint: run the reconcile controller (needs `--features
    /// operator`). The default when no subcommand is given.
    Run,
    /// Render operator namespace/RBAC/deployment YAML.
    Render(K8sOperatorRenderArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct K8sOperatorRenderArgs {
    /// Namespace that owns the operator control plane.
    #[arg(long, default_value = "relay-system")]
    pub(crate) namespace: String,
    /// Write to this path instead of stdout. A directory receives
    /// `operator.yaml`.
    #[arg(long)]
    pub(crate) out: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct K8sInstanceArgs {
    #[command(subcommand)]
    pub(crate) cmd: K8sInstanceCmd,
}

#[derive(Subcommand, Debug)]
pub(crate) enum K8sInstanceCmd {
    /// Render a namespaced `kind: Relay` custom resource.
    Render(K8sInstanceRenderArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct K8sInstanceRenderArgs {
    /// Built-in instance profile.
    #[arg(long, value_enum, default_value_t = K8sInstanceProfile::Dev)]
    pub(crate) profile: K8sInstanceProfile,
    /// Relay CR name. HA (replicasPerShard > 1) instances must keep the
    /// default `relay` — serve derives raft peer DNS as
    /// `relay-<ordinal>.<peer-service>`.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Namespace where the app-facing Relay instance lives.
    #[arg(long)]
    pub(crate) namespace: Option<String>,
    /// Broker image. Defaults are profile-specific.
    #[arg(long)]
    pub(crate) image: Option<String>,
    /// Write to this path instead of stdout. A directory receives `relay.yaml`.
    #[arg(long)]
    pub(crate) out: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum K8sInstanceProfile {
    /// Small local/kind CR: one broker pod, small disk, verbose logs.
    Dev,
    /// Pre-prod CR: prod-shaped single node, info logs, mid disk.
    Staging,
    /// Production-shape CR: 3-replica raft-HA group, large disk, auth on.
    Prod,
    /// Fill-in-the-blanks CR skeleton for app teams.
    Template,
}

#[derive(clap::Args, Debug)]
pub(crate) struct K8sFileOutputArgs {
    /// Write to this path instead of stdout.
    #[arg(long)]
    pub(crate) out: Option<PathBuf>,
}

/// `relay k8s` — cluster artifacts split by lifecycle layer. Only `operator
/// run` needs kube-rs at runtime; the render paths are offline and work from
/// the binary (the generated CRD is embedded, the operator manifests are
/// string-templated, the instance CRs are profile-templated).
pub(crate) async fn run(args: K8sArgs) -> Result<()> {
    match args.cmd {
        K8sCmd::Crd(a) => match a.cmd {
            K8sCrdCmd::Render(a) => write_or_print(a.out.as_deref(), "crd.yaml", &crd_yaml()),
        },
        K8sCmd::Operator(a) => match a.cmd.unwrap_or(K8sOperatorCmd::Run) {
            K8sOperatorCmd::Run => run_operator().await,
            K8sOperatorCmd::Render(a) => {
                let yaml = render_operator_yaml(&a.namespace);
                write_or_print(a.out.as_deref(), "operator.yaml", &yaml)
            }
        },
        K8sCmd::Instance(a) => match a.cmd {
            K8sInstanceCmd::Render(a) => {
                let yaml = render_instance_yaml(&a);
                write_or_print(a.out.as_deref(), "relay.yaml", &yaml)
            }
        },
    }
}

#[cfg(feature = "operator")]
async fn run_operator() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    relay::operator::run().await
}

#[cfg(not(feature = "operator"))]
async fn run_operator() -> Result<()> {
    anyhow::bail!(
        "this relay build was compiled without operator support; rebuild with \
         `--features operator` (the published image includes it)"
    )
}

#[cfg(feature = "operator")]
fn crd_yaml() -> String {
    relay::operator::crd_yaml()
}

#[cfg(not(feature = "operator"))]
fn crd_yaml() -> String {
    cli_std::artifact::ensure_trailing_newline(include_str!("../../../../../k8s/operator/crd.yaml"))
}

/// Render the operator control-plane manifests (RBAC + Deployment) with the
/// namespace substituted, from the checked-in fixtures.
fn render_operator_yaml(namespace: &str) -> String {
    let mut out = String::new();
    out.push_str(&cli_std::artifact::replace_kubernetes_namespace(
        include_str!("../../../../../k8s/operator/rbac.yaml"),
        "relay-system",
        namespace,
    ));
    out.push_str("\n---\n");
    out.push_str(&cli_std::artifact::replace_kubernetes_namespace(
        include_str!("../../../../../k8s/operator/deployment.yaml"),
        "relay-system",
        namespace,
    ));
    cli_std::artifact::ensure_trailing_newline(&out)
}

/// Render a `kind: Relay` custom resource for the selected profile.
fn render_instance_yaml(args: &K8sInstanceRenderArgs) -> String {
    let default_version = env!("CARGO_PKG_VERSION");
    let (default_name, default_namespace, default_image, body) = match args.profile {
        K8sInstanceProfile::Dev => (
            "relay",
            "default",
            "relay:latest".to_string(),
            InstanceBody::Dev,
        ),
        K8sInstanceProfile::Staging => (
            "relay",
            "staging",
            format!("relay:{default_version}"),
            InstanceBody::Staging,
        ),
        K8sInstanceProfile::Prod => (
            "relay",
            "production",
            format!("registry.example.com/relay:{default_version}"),
            InstanceBody::Prod,
        ),
        K8sInstanceProfile::Template => (
            "relay",
            "REPLACE_ME__APP_NAMESPACE",
            "REPLACE_ME__REGISTRY/relay:REPLACE_ME__IMAGE_TAG".to_string(),
            InstanceBody::Template,
        ),
    };
    let name = args.name.as_deref().unwrap_or(default_name);
    let namespace = args.namespace.as_deref().unwrap_or(default_namespace);
    let image = args.image.as_deref().unwrap_or(&default_image);

    let mut yaml = format!(
        "apiVersion: relay.dev/v1alpha1\nkind: Relay\nmetadata:\n  name: {name}\n  namespace: {namespace}\nspec:\n  image: {image}\n"
    );
    match body {
        InstanceBody::Dev => {
            yaml.push_str(
                "  replicasPerShard: 1\n  voterCount: 1\n  logLevel: debug\n  storage: 1Gi\n  resources:\n    cpu: \"250m\"\n    memory: 256Mi\n",
            );
        }
        InstanceBody::Staging => {
            yaml.push_str(
                "  replicasPerShard: 1\n  voterCount: 1\n  logLevel: info\n  storage: 20Gi\n  resources:\n    cpu: \"1\"\n    memory: 2Gi\n",
            );
        }
        InstanceBody::Prod => {
            yaml.push_str(
                "  imagePullPolicy: Always\n  replicasPerShard: 3\n  voterCount: 3\n  logLevel: info\n  storage: 100Gi\n  graceSecs: 30\n  auth: required\n  tokensSecret: relay-token-registry\n  resources:\n    cpu: \"4\"\n    memory: 8Gi\n",
            );
        }
        InstanceBody::Template => {
            yaml.push_str(
                "  imagePullPolicy: IfNotPresent\n  replicasPerShard: REPLACE_ME__REPLICAS_PER_SHARD\n  voterCount: REPLACE_ME__VOTER_COUNT\n  storage: 10Gi\n  resources:\n    cpu: \"1\"\n    memory: 1Gi\n",
            );
        }
    }
    cli_std::artifact::ensure_trailing_newline(&yaml)
}

pub(crate) enum InstanceBody {
    Dev,
    Staging,
    Prod,
    Template,
}
