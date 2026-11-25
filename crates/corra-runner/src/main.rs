use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "corra-runner",
    version,
    about = "Runs corra's agents on a machine"
)]
struct Cli {
    #[arg(long, env = "CORRA_HUB")]
    hub: String,
    #[arg(long, env = "CORRA_TOKEN_FILE")]
    token_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "corra_runner=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let token = match (&cli.token_file, std::env::var("CORRA_TOKEN")) {
        (Some(p), _) => std::fs::read_to_string(p)
            .with_context(|| format!("reading {}", p.display()))?
            .trim()
            .to_string(),
        (None, Ok(t)) => t,
        (None, Err(_)) => return Err(anyhow!("set --token-file or CORRA_TOKEN")),
    };
    corra_runner::run(corra_runner::Options {
        hub: cli.hub,
        token,
        sync: true,
        update: true,
        connected: None,
    })
    .await
}
