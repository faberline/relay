//! `relay upgrade`: self-update this binary from a published release.

use anyhow::Result;

use crate::llm::TOOL;

/// `relay upgrade` flags.
#[derive(clap::Args, Debug)]
pub(crate) struct UpgradeArgs {
    /// Report the current and latest version without modifying the binary.
    #[arg(long)]
    pub(crate) check: bool,
    /// Install this exact version (`0.4.3` or `relay@0.4.3`) instead of the latest.
    #[arg(long)]
    pub(crate) tag: Option<String>,
    /// Reinstall even when already on the selected version.
    #[arg(long)]
    pub(crate) force: bool,
    /// Skip the confirmation prompt.
    #[arg(short = 'y', long)]
    pub(crate) yes: bool,
}

/// `relay upgrade` — self-update from a published GitHub release.
pub(crate) async fn run(args: UpgradeArgs) -> Result<()> {
    cli_std::upgrade::run(
        &TOOL,
        cli_std::upgrade::Options {
            check: args.check,
            tag: args.tag,
            force: args.force,
            yes: args.yes,
        },
    )
    .await
}
