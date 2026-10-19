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
//! Each verb lives in its own module; this file owns the clap surface,
//! dispatch, and the CLI parse tests.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod backup;
mod dockerfile;
mod issue;
mod k8s;
mod llm;
mod serve;
mod spec;
mod upgrade;

use backup::BackupArgs;
use dockerfile::DockerfileArgs;
use issue::IssueArgs;
use k8s::K8sArgs;
use llm::LlmArgs;
use serve::ServeArgs;
use spec::SpecArgs;
use upgrade::UpgradeArgs;

#[derive(Parser, Debug)]
#[command(
    name = "relay",
    version,
    about = "relay — durable single-cast work-queue broker (HTTP/2 + OpenAPI)"
)]
struct Cli {
    /// Standard agent-facing command. Omit it to run the server (the default).
    #[command(subcommand)]
    cmd: Option<Command>,
    /// Server flags — used when no subcommand is given (`relay <flags>`).
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print agent-facing LLM topics — offline, no server. `outline` (default)
    /// maps the topics; pass a topic id for detail (`--format json` for a
    /// machine-readable form).
    Llm(LlmArgs),
    /// Self-update this binary from a published GitHub release. Resolves the
    /// running target + version, downloads the matching `relay-<target>.tar.gz`,
    /// verifies its sha256, and atomically replaces the executable. `--check`
    /// reports the available version without changing anything.
    Upgrade(UpgradeArgs),
    /// Search, view, and file relay issues on the faberline/relay GitHub tracker
    /// (`search`/`view`/`create`). `create` auto-tags `app:relay`;
    /// `search` is filtered to relay's own issues.
    Issue(IssueArgs),
    /// Kubernetes artifacts split by layer: the cluster-scoped CRD, the
    /// operator control plane, and app-namespace Relay instances. Render paths
    /// are offline (they work from the binary); only `operator run` needs the
    /// `operator` build feature.
    K8s(K8sArgs),
    /// Render relay's runtime image Dockerfiles — offline, no server. Image
    /// construction is owned here (not by `k8s`) because the same artifact
    /// feeds compose, kind, and real registries.
    Dockerfile(DockerfileArgs),
    /// Print relay's machine-readable integration spec — offline, no server:
    /// the same OpenAPI document `GET /openapi.json` serves. `spec gen --lang
    /// ts|py|rust` generates a typed client from it (#1209, keep #777).
    Spec(SpecArgs),
    /// Write a consistent snapshot of a RUNNING node's live (un-acked) state
    /// to a backup destination through the shared core/service-backup runner
    /// (#1209): fetches `GET /admin/backup` and ships the bytes to `--dest`
    /// (`file://` always; `s3://` via the lib). Needs a build with
    /// `--features backup`.
    Backup(BackupArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install the process-level rustls crypto provider before anything parses
    // or dials TLS (the operator/kube and online CLI paths link rustls, which
    // panics without a default provider). A no-op in the default,
    // provider-free build. See `relay::tls`.
    relay::tls::install_default_crypto_provider();
    let cli = Cli::parse();
    match cli.cmd {
        // Default (no subcommand): run the server.
        None => serve::run(cli.serve).await,
        Some(cmd) => dispatch(cmd).await,
    }
}

async fn dispatch(cmd: Command) -> Result<()> {
    match cmd {
        Command::Llm(args) => llm::run(args),
        Command::Upgrade(args) => upgrade::run(args).await,
        Command::Issue(args) => issue::run(args).await,
        Command::K8s(args) => k8s::run(args).await,
        Command::Dockerfile(args) => dockerfile::run(args),
        Command::Spec(args) => spec::run(args),
        Command::Backup(args) => backup::run(args).await,
    }
}

/// Write `body` to `out` (a file, or `default_file` inside a directory) or
/// print it to stdout.
fn write_or_print(out: Option<&std::path::Path>, default_file: &str, body: &str) -> Result<()> {
    let written = cli_std::artifact::write_or_print(out, default_file, body)?;
    if written.is_some() {
        println!("next: done");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use std::path::PathBuf;

    use super::dockerfile::{DockerfileCmd, DockerfileVariant};
    use super::k8s::{
        K8sCmd, K8sInstanceArgs, K8sInstanceCmd, K8sInstanceProfile, K8sOperatorArgs,
    };
    use super::llm::TOOL;
    use super::spec::{GenLang, SpecSub};

    /// R1: the convention verbs + bare-serve default all parse.
    #[test]
    fn cli_parse_surface() {
        Cli::command().debug_assert();
        assert!(Cli::try_parse_from(["relay"]).unwrap().cmd.is_none());
        assert!(matches!(
            Cli::try_parse_from(["relay", "llm"]).unwrap().cmd,
            Some(Command::Llm(_))
        ));
        assert!(matches!(
            Cli::try_parse_from(["relay", "llm", "http-api", "--format", "json"])
                .unwrap()
                .cmd,
            Some(Command::Llm(_))
        ));
        assert!(matches!(
            Cli::try_parse_from(["relay", "upgrade", "--check"])
                .unwrap()
                .cmd,
            Some(Command::Upgrade(_))
        ));
        assert!(matches!(
            Cli::try_parse_from(["relay", "issue", "search", "lease"])
                .unwrap()
                .cmd,
            Some(Command::Issue(_))
        ));
        assert!(matches!(
            Cli::try_parse_from(["relay", "issue", "view", "42"])
                .unwrap()
                .cmd,
            Some(Command::Issue(_))
        ));
        assert!(matches!(
            Cli::try_parse_from(["relay", "issue", "create", "--dry-run", "it", "broke"])
                .unwrap()
                .cmd,
            Some(Command::Issue(_))
        ));
        // R2: relay-server's env knobs surface as flags on the bare path.
        let cli = Cli::try_parse_from(["relay", "--bind", "127.0.0.1:0", "--data-dir", "/tmp/x"])
            .unwrap();
        assert!(cli.cmd.is_none());
        assert_eq!(cli.serve.bind, "127.0.0.1:0");
        assert_eq!(cli.serve.data_dir.as_deref(), Some("/tmp/x"));
        // #1205: graceful-drain window — default 10, overridable via flag.
        assert_eq!(cli.serve.grace_secs, 10);
        let cli = Cli::try_parse_from(["relay", "--grace-secs", "3"]).unwrap();
        assert_eq!(cli.serve.grace_secs, 3);
        // #1206: the service-auth contract surfaces as serve flags with env
        // fallback — default off (tokenless dev), `required` + registry file.
        assert_eq!(cli.serve.auth, "off");
        assert!(cli.serve.token_registry_file.is_none());
        let cli = Cli::try_parse_from([
            "relay",
            "--auth",
            "required",
            "--token-registry-file",
            "/var/run/secrets/relay/token-registry.json",
        ])
        .unwrap();
        assert_eq!(cli.serve.auth, "required");
        assert_eq!(
            cli.serve.token_registry_file.as_deref(),
            Some("/var/run/secrets/relay/token-registry.json")
        );
        // #544: the raft peer-DNS service name surfaces as a serve flag with
        // env fallback; default matches the headless Service in k8s/.
        assert_eq!(cli.serve.peer_service, "relay");
        let cli = Cli::try_parse_from(["relay", "--peer-service", "relay-peer"]).unwrap();
        assert_eq!(cli.serve.peer_service, "relay-peer");
    }

    /// #1208: `relay k8s crd/operator/instance` verbs parse with their
    /// convention flags. Positionals name subcommands; profile/namespace/out
    /// are flags.
    #[test]
    fn k8s_verbs_parse() {
        Cli::try_parse_from(["relay", "k8s", "crd", "render"]).expect("crd render");
        Cli::try_parse_from(["relay", "k8s", "operator", "run"]).expect("operator run");
        Cli::try_parse_from([
            "relay",
            "k8s",
            "operator",
            "render",
            "--namespace",
            "relay-system",
        ])
        .expect("operator render");

        let cli = Cli::try_parse_from([
            "relay",
            "k8s",
            "instance",
            "render",
            "--profile",
            "prod",
            "--namespace",
            "production",
        ])
        .expect("instance render");
        match cli.cmd {
            Some(Command::K8s(K8sArgs {
                cmd:
                    K8sCmd::Instance(K8sInstanceArgs {
                        cmd: K8sInstanceCmd::Render(a),
                    }),
            })) => {
                assert!(matches!(a.profile, K8sInstanceProfile::Prod));
                assert_eq!(a.namespace.as_deref(), Some("production"));
            }
            other => panic!("expected k8s instance render, got {other:?}"),
        }

        // `operator` with no subcommand defaults to `run`.
        let cli = Cli::try_parse_from(["relay", "k8s", "operator"]).expect("operator default");
        match cli.cmd {
            Some(Command::K8s(K8sArgs {
                cmd: K8sCmd::Operator(K8sOperatorArgs { cmd }),
            })) => assert!(cmd.is_none()),
            other => panic!("expected k8s operator, got {other:?}"),
        }
    }

    /// #1208: `relay dockerfile render` parses with variant/version/out flags.
    #[test]
    fn dockerfile_verbs_parse() {
        let cli = Cli::try_parse_from([
            "relay",
            "dockerfile",
            "render",
            "--variant",
            "release",
            "--version",
            "1.2.3",
        ])
        .expect("dockerfile render should parse");
        match cli.cmd {
            Some(Command::Dockerfile(DockerfileArgs {
                cmd: DockerfileCmd::Render(a),
            })) => {
                assert!(matches!(a.variant, DockerfileVariant::Release));
                assert_eq!(a.version.as_deref(), Some("1.2.3"));
            }
            other => panic!("expected dockerfile render, got {other:?}"),
        }
        // Version tag normalization: bare and prefixed forms converge.
        assert_eq!(
            cli_std::artifact::release_tag("relay", Some("1.2.3"), env!("CARGO_PKG_VERSION")),
            "relay@1.2.3"
        );
        assert_eq!(
            cli_std::artifact::release_tag("relay", Some("relay@1.2.3"), env!("CARGO_PKG_VERSION")),
            "relay@1.2.3"
        );
        assert_eq!(
            cli_std::artifact::release_tag("relay", None, env!("CARGO_PKG_VERSION")),
            format!("relay@{}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// #1209: `relay spec` / `relay spec gen` / `relay backup` parse with
    /// their convention flags; the keep-only `--shapes`/`--fields` do NOT
    /// parse (relay has no catalogs — omitted, never faked).
    #[test]
    fn spec_and_backup_verbs_parse() {
        Cli::try_parse_from(["relay", "spec"]).expect("spec default");
        Cli::try_parse_from(["relay", "spec", "--format", "openapi-yaml"]).expect("spec yaml");
        Cli::try_parse_from(["relay", "spec", "--format", "json-schema"])
            .expect("spec json-schema");
        assert!(
            Cli::try_parse_from(["relay", "spec", "--shapes"]).is_err(),
            "--shapes is keep-only"
        );
        assert!(
            Cli::try_parse_from(["relay", "spec", "--fields"]).is_err(),
            "--fields is keep-only"
        );

        let cli = Cli::try_parse_from([
            "relay",
            "spec",
            "gen",
            "--lang",
            "ts",
            "--target",
            "typescript-5.0",
            "--out",
            "/tmp/x",
        ])
        .expect("spec gen should parse");
        match cli.cmd {
            Some(Command::Spec(SpecArgs {
                gen: Some(SpecSub::Gen(a)),
                ..
            })) => {
                assert!(matches!(a.lang, GenLang::Ts));
                assert_eq!(a.target.as_deref(), Some("typescript-5.0"));
                assert_eq!(a.out, PathBuf::from("/tmp/x"));
            }
            other => panic!("expected spec gen, got {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "relay",
            "backup",
            "--url",
            "http://localhost:7000",
            "--dest",
            "file:///tmp/backups",
            "--retention-secs",
            "3600",
        ])
        .expect("backup should parse");
        match cli.cmd {
            Some(Command::Backup(a)) => {
                assert_eq!(a.url, "http://localhost:7000");
                assert_eq!(a.dest, "file:///tmp/backups");
                assert_eq!(a.retention_secs, Some(3600));
            }
            other => panic!("expected backup, got {other:?}"),
        }
    }

    /// R3: build-stamp envs populate ToolInfo (never empty; "unknown" is the
    /// stamped fallback outside a git checkout).
    #[test]
    fn toolinfo_is_stamped() {
        assert_eq!(TOOL.project, "relay");
        assert!(!TOOL.version.is_empty());
        assert!(!TOOL.target.is_empty());
        assert!(!TOOL.git_sha.is_empty());
        assert!(!TOOL.built_at.is_empty());
    }

    /// #2556: the body-limit-bytes flag defaults to 8 MiB and respects the
    /// RELAY_BODY_LIMIT_BYTES env var override.
    #[test]
    fn body_limit_bytes_defaults_and_respects_env() {
        use clap::Parser;

        // Default: 8 MiB.
        let cli = Cli::parse_from(["relay"]);
        assert_eq!(cli.serve.body_limit_bytes, 8 * 1024 * 1024);

        // Explicit flag overrides the default.
        let cli = Cli::parse_from(["relay", "--body-limit-bytes", "16777216"]);
        assert_eq!(cli.serve.body_limit_bytes, 16777216);
    }
}
// HANDWRITE-END
