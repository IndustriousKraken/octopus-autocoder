use anyhow::Result;
use clap::Parser;

mod cli;
mod config;
mod executor;
mod git;
mod github;
mod polling_loop;
mod queue;
mod workspace;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = cli::Cli::parse();
    cli::dispatch(cli).await
}
