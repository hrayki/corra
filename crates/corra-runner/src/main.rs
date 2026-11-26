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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("mcp-bridge") {
        let path = args
            .get(2)
            .ok_or_else(|| anyhow!("mcp-bridge needs a socket path"))?;
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(corra_runner::agents::mcp::bridge(path));
    }
    let cli = Cli::parse();
    #[cfg(target_os = "linux")]
    if std::env::var_os(corra_runner::reaper::CHILD_ENV).is_none() {
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.args(std::env::args_os().skip(1))
            .env(corra_runner::reaper::CHILD_ENV, "1");
        std::process::exit(corra_runner::reaper::supervise(cmd)?);
    }
    run(cli)
}

#[tokio::main]
async fn run(cli: Cli) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "corra_runner=info".into()),
        )
        .init();
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
