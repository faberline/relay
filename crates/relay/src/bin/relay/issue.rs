//! `relay issue`: search, view, and file relay issues via cli-std.

use anyhow::Result;
use clap::Subcommand;

use crate::llm::TOOL;

/// `relay issue <search|view|create>` — search, read, and file relay issues.
/// Positional slots are reserved for the verb + its primary object, so the rest
/// are flags (the CLI convention).
#[derive(clap::Args, Debug)]
pub(crate) struct IssueArgs {
    #[command(subcommand)]
    pub(crate) cmd: IssueCommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum IssueCommand {
    /// Search relay's issues (`app:relay`); omit the query to list recent.
    Search(IssueSearchArgs),
    /// Print a single issue by number.
    View(IssueViewArgs),
    /// File a structured issue (auto-tagged `app:relay`).
    Create(IssueCreateArgs),
}

/// `relay issue search [query] [--state] [--limit]` flags.
#[derive(clap::Args, Debug)]
pub(crate) struct IssueSearchArgs {
    /// Search text (omit to list recent issues).
    #[arg(num_args = 0..)]
    pub(crate) query: Vec<String>,
    /// Issue state filter.
    #[arg(long, value_parser = ["open", "closed", "all"], default_value = "open")]
    pub(crate) state: String,
    /// Max results.
    #[arg(long, default_value_t = 20)]
    pub(crate) limit: u32,
}

/// `relay issue view <number>` flags.
#[derive(clap::Args, Debug)]
pub(crate) struct IssueViewArgs {
    /// Issue number.
    pub(crate) number: u64,
}

/// `relay issue create [--title <t>] [message...]` flags.
#[derive(clap::Args, Debug)]
pub(crate) struct IssueCreateArgs {
    /// Issue title (default: derived from the message).
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// Print the issue that would be filed (and its URL) without creating it.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Free-text description of the problem.
    #[arg(num_args = 0..)]
    pub(crate) message: Vec<String>,
}

/// `relay issue <verb>` — dispatch search/view/create to cli-std. `create`
/// always tags `app:relay`; `search` is filtered to relay's own issues.
pub(crate) async fn run(args: IssueArgs) -> Result<()> {
    match args.cmd {
        IssueCommand::Search(m) => {
            let joined = m.query.join(" ");
            let query = (!joined.trim().is_empty()).then_some(joined);
            cli_std::issue::search(
                &TOOL,
                cli_std::issue::SearchOptions {
                    query,
                    state: m.state,
                    limit: m.limit,
                },
            )
            .await
        }
        IssueCommand::View(m) => cli_std::issue::view(&TOOL, m.number).await,
        IssueCommand::Create(m) => {
            let msg = m.message.join(" ");
            let title = m.title.unwrap_or_else(|| {
                if msg.trim().is_empty() {
                    "relay: issue report".to_string()
                } else {
                    let head: String = msg.lines().next().unwrap_or("").chars().take(72).collect();
                    format!("relay: {head}")
                }
            });
            let message = (!msg.trim().is_empty()).then_some(msg);
            cli_std::issue::create(
                &TOOL,
                cli_std::issue::CreateOptions {
                    title,
                    message,
                    url: None,
                    repo: None,
                    label: vec!["app:relay".to_string()],
                    dry_run: m.dry_run,
                    yes: true,
                },
            )
            .await
        }
    }
}
