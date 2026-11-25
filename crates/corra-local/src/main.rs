use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "corra-local",
    version,
    about = "Corra on this computer, without the desktop window"
)]
struct Cli {
    #[arg(long, env = "CORRA_LOCAL_DATA")]
    data: Option<PathBuf>,
    #[arg(long, default_value_t = 7811)]
    port: u16,
    #[arg(long)]
    url_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "corra_local=info,corra_hub=info,corra_runner=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let mut desktop = corra_local::start(corra_local::Options {
        data: cli.data.unwrap_or_else(corra_local::default_data_dir),
        port: cli.port,
    })
    .await?;
    println!("corra's built-in hub is at {}", desktop.base());
    let mut notices = desktop.notices();
    loop {
        tokio::select! {
            Ok(n) = notices.recv() => {
                println!("notification: {} — {}", n["title"].as_str().unwrap_or(""), n["body"].as_str().unwrap_or(""));
            }
            changed = desktop.target.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let target = desktop.target.borrow_and_update().clone();
                let Some(url) = target.url() else { continue };
                match &cli.url_file {
                    Some(f) => {
                        use std::os::unix::fs::OpenOptionsExt;
                        let _ = std::fs::remove_file(f);
                        let mut file = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .mode(0o600)
                            .open(f)?;
                        std::io::Write::write_all(&mut file, url.as_bytes())?;
                        println!("the window would show {:?}; its address is in {}", target_kind(&target), f.display());
                    }
                    None => println!("the window would show {url}"),
                }
            }
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}

fn target_kind(t: &corra_local::Target) -> &'static str {
    match t {
        corra_local::Target::Starting => "nothing",
        corra_local::Target::Connect(_) => "the connect page",
        corra_local::Target::Hub(_) => "the hub",
        corra_local::Target::Offline(_) => "the built-in hub",
    }
}
