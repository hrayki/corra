use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use corra_hub::db::{Db, NewMachine};
use corra_hub::hub::{hash_token, new_token};
use corra_hub::{auth, secrets};
use corra_proto::MachineStatus;

#[derive(Parser)]
#[command(
    name = "corra-hub",
    version,
    about = "Corra's hub: machines, threads, credentials and tools"
)]
struct Cli {
    #[arg(long, env = "CORRA_DATA", default_value = "./data")]
    data: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Serve {
        #[arg(long, env = "CORRA_LISTEN")]
        listen: Option<SocketAddr>,
        #[arg(long, env = "CORRA_PUBLIC_URL")]
        public_url: Option<String>,
        #[arg(long, env = "CORRA_DEV_NO_AUTH", default_value_t = false)]
        dev_no_auth: bool,
    },
    Machine {
        #[command(subcommand)]
        cmd: MachineCmd,
    },
    Secret {
        #[command(subcommand)]
        cmd: SecretCmd,
    },
    Device {
        #[command(subcommand)]
        cmd: DeviceCmd,
    },
}

#[derive(Subcommand)]
enum DeviceCmd {
    Add {
        name: String,
        #[arg(long)]
        token_file: PathBuf,
    },
}

#[derive(Subcommand)]
enum SecretCmd {
    Set { name: String },
    Delete { name: String },
    List,
}

#[derive(Subcommand)]
enum MachineCmd {
    Add {
        name: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "corra_hub=info".into()),
        )
        .init();
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.data)
        .with_context(|| format!("creating {}", cli.data.display()))?;
    if let Cmd::Serve {
        listen,
        public_url,
        dev_no_auth,
    } = cli.cmd
    {
        return serve(cli.data, listen, public_url, dev_no_auth).await;
    }
    let db = Db::open(&cli.data.join("corra.db"))?;
    match cli.cmd {
        Cmd::Serve { .. } => unreachable!(),
        Cmd::Machine {
            cmd: MachineCmd::Add { name, token_file },
        } => {
            if token_file.exists() {
                bail!("{} already exists", token_file.display());
            }
            let token = new_token("cm");
            let m = db.insert_machine(NewMachine {
                name,
                provider: "existing".into(),
                project: None,
                size: "existing".into(),
                token_hash: hash_token(&token),
                provider_ref: None,
                status: MachineStatus::Offline,
                kind: None,
            })?;
            write_private(&token_file, &token)?;
            println!(
                "machine {} ({}); its token is in {}",
                m.name,
                m.id,
                token_file.display()
            );
            Ok(())
        }
        Cmd::Secret { cmd } => {
            let secrets = secrets::Secrets::open(db, &cli.data)?;
            match cmd {
                SecretCmd::Set { name } => {
                    let mut value = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut value)?;
                    if value.trim().is_empty() {
                        bail!("give the value on stdin");
                    }
                    secrets.set(&name, value.trim())?;
                    println!("saved {name}");
                }
                SecretCmd::Delete { name } => {
                    secrets.delete(&name)?;
                    println!("deleted {name}");
                }
                SecretCmd::List => {
                    for n in secrets.names()? {
                        println!("{n}");
                    }
                }
            }
            Ok(())
        }
        Cmd::Device {
            cmd: DeviceCmd::Add { name, token_file },
        } => {
            if token_file.exists() {
                bail!("{} already exists", token_file.display());
            }
            let token = auth::create_device_session(&db, &name)?;
            write_private(&token_file, &token)?;
            println!("device {name}; its token is in {}", token_file.display());
            Ok(())
        }
        Cmd::Machine {
            cmd: MachineCmd::List,
        } => {
            for m in db.machines()? {
                println!(
                    "{}  {}  {}  {}",
                    m.id,
                    m.name,
                    m.provider,
                    m.status.as_str()
                );
            }
            Ok(())
        }
    }
}

fn check_public_url(u: &str) -> Result<String> {
    let u = u.trim().trim_end_matches('/').to_string();
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .ok_or_else(|| anyhow::anyhow!("CORRA_PUBLIC_URL must start with https://"))?;
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
        bail!("CORRA_PUBLIC_URL must be the hub's address alone, like https://corra.example.com");
    }
    Ok(u)
}

fn write_private(path: &std::path::Path, content: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(content.as_bytes())?;
    Ok(())
}

async fn serve(
    data: PathBuf,
    listen: Option<SocketAddr>,
    public_url: Option<String>,
    dev_no_auth: bool,
) -> Result<()> {
    let listen = match (listen, std::env::var("PORT").ok()) {
        (Some(l), _) => l,
        (None, Some(port)) => format!("0.0.0.0:{}", port.trim())
            .parse()
            .context("PORT must be a port number")?,
        (None, None) => "127.0.0.1:7810".parse()?,
    };
    if dev_no_auth && !listen.ip().is_loopback() {
        bail!("--dev-no-auth only works on a loopback address");
    }
    let public_url = match public_url {
        Some(u) => check_public_url(&u)?,
        None if listen.ip().is_loopback() => format!("http://localhost:{}", listen.port()),
        None => bail!(
            "set CORRA_PUBLIC_URL to the https address the hub is reached at: machines and the apps need it"
        ),
    };
    let storage = corra_hub::storage::from_env()?;
    if let Some(st) = &storage
        && std::env::var_os(corra_hub::storage::CHILD).is_none()
    {
        return corra_hub::storage::run_under_litestream(st.clone(), &data, listen).await;
    }
    let built = corra_hub::build(corra_hub::Options {
        data: data.clone(),
        public_url,
        dev_no_auth,
    })?;
    let app = corra_hub::finish(built.router);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("hub listening on {listen}");
    let holder = storage
        .as_ref()
        .map(|st| corra_hub::storage::hold(st.clone()));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        r = axum::serve(listener, app).into_future() => r?,
        _ = term.recv() => tracing::info!("stopping"),
        _ = tokio::signal::ctrl_c() => tracing::info!("stopping"),
    }
    if let (Some(st), Some(h)) = (&storage, &holder) {
        corra_hub::storage::hand_over(st, h, &data).await;
    }
    Ok(())
}
