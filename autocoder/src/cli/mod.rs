//! CLI argument parsing + dispatch. Each subcommand's execute function lives
//! in its own submodule.

use crate::config;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod audit;
pub mod check_config;
pub mod install;
pub mod reload;
pub mod rewind;
pub mod run;
pub mod sync_specs;
pub mod sync_specs_deps;

#[derive(Parser, Debug)]
#[command(name = "autocoder")]
#[command(about = "Autonomous AI code-writer driven by OpenSpec changes", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum AuditSubcommand {
    /// Trigger an audit for a workspace. With the daemon running, the
    /// CLI sends a `queue_audit` action via the control socket so the
    /// next polling iteration runs the audit. Without the daemon, the
    /// audit module is invoked directly against the workspace and
    /// findings print to stdout.
    Run {
        /// Path to the workspace directory.
        #[arg(long)]
        workspace: PathBuf,

        /// Audit type name (e.g. `security_bug_audit`). The exact
        /// `audit_type` slug — substring matching is reserved for the
        /// chatops verb.
        #[arg(long)]
        audit: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the autocoder daemon. Polls every configured repository on its
    /// interval, processes ready OpenSpec changes, and opens monolithic PRs.
    Run {
        /// Path to the YAML configuration file.
        #[arg(long)]
        config: PathBuf,
    },

    /// Validate a config file against this binary's schema, without
    /// running the daemon. Exits 0 on a clean config, 1 on
    /// warnings-only (typically unset env vars referenced by `*_env`
    /// fields), 2 on at least one hard error. Use this as a CI gate
    /// AND as the preflight before `update.sh` swaps a new binary into
    /// place.
    CheckConfig(check_config::CheckConfigArgs),

    /// Internal: stdio MCP server exposing the `ask_user` tool. Launched
    /// by the wrapped CLI agent (via the workspace's `.mcp.json` config),
    /// NOT invoked directly by humans.
    #[command(hide = true)]
    McpAskUserServer,

    /// Reload the running daemon's hot-applicable config sections (github,
    /// reviewer, chatops) from the on-disk YAML the daemon was launched
    /// with. Connects to the daemon's control socket; exits non-zero if
    /// the daemon is not running or the new YAML fails validation.
    Reload,

    /// First-run wizard. Collects the minimum configuration an operator
    /// needs (one repo URL, a GitHub PAT, optional chatops + reviewer),
    /// writes config.yaml + secrets.env, and on server mode renders +
    /// enables a systemd unit. Idempotent: re-running against an existing
    /// config prints a status line and exits 0.
    Install(install::InstallArgs),

    /// Rebuild every canonical spec under `openspec/specs/` from the
    /// archived change history under `openspec/changes/archive/`. The
    /// rebuild iterates archives chronologically and replays each via
    /// `openspec archive` so canonical state is exactly what it would be
    /// if every archive had synced correctly the first time. See the
    /// "Rebuilding canonical specs" section of the README for the
    /// operator's perspective.
    SyncSpecs {
        /// Path to the workspace (the directory containing
        /// `openspec/changes/archive/`).
        #[arg(long)]
        workspace: PathBuf,

        /// Run the full rebuild. There is no incremental mode; this
        /// flag exists for clarity and future-proofing. Defaults to
        /// true.
        #[arg(long, default_value_t = true)]
        rebuild: bool,

        /// SIGTERM the running executor subprocess (if any) before
        /// starting the rebuild. Without this flag the CLI waits
        /// politely for the current iteration to finish. No-op when
        /// no daemon is running on the workspace.
        #[arg(long, default_value_t = false)]
        immediate: bool,
    },

    /// On-demand audit triggers (chatops-on-demand-audit-trigger). The
    /// `run` subcommand queues an audit for the daemon's next polling
    /// iteration when the daemon is reachable, OR invokes the audit
    /// module directly against the named workspace when no daemon is
    /// running (useful for prompt-template iteration).
    Audit {
        #[command(subcommand)]
        command: AuditSubcommand,
    },

    /// Recover from a failed PR or bad implementation by unarchiving named
    /// changes and resetting the agent branch.
    Rewind {
        /// One or more change names to unarchive.
        #[arg(required = true)]
        changes: Vec<String>,

        /// Path to the YAML configuration file.
        #[arg(long)]
        config: PathBuf,

        /// Skip the confirmation prompt and force-delete the agent branch
        /// locally and remotely.
        #[arg(long, default_value_t = false)]
        hard: bool,

        /// Repository URL or short-name (basename without .git). Required
        /// when config has multiple repositories.
        #[arg(long)]
        repo: Option<String>,
    },
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Run { config } => {
            let cfg = config::Config::load_from(&config)?;
            run::execute(cfg, config).await
        }
        Command::CheckConfig(args) => check_config::execute(args).await,
        Command::Install(args) => install::execute(args).await,
        Command::Reload => reload::execute().await,
        Command::McpAskUserServer => crate::mcp_askuser_server::run(),
        Command::SyncSpecs {
            workspace,
            rebuild,
            immediate,
        } => {
            sync_specs::execute(sync_specs::SyncSpecsArgs {
                workspace,
                rebuild,
                immediate,
            })
            .await
        }
        Command::Audit { command } => match command {
            AuditSubcommand::Run { workspace, audit } => {
                audit::execute(workspace, audit).await
            }
        },
        Command::Rewind {
            changes,
            config: config_path,
            hard,
            repo,
        } => {
            let cfg = config::Config::load_from(&config_path)?;
            rewind::execute(
                cfg.repositories,
                cfg.github,
                rewind::RewindArgs {
                    changes,
                    hard,
                    repo,
                },
            )
            .await
        }
    }
}
