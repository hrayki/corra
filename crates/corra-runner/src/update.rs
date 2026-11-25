use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub fn own_build() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let bytes = std::fs::read(exe).ok()?;
    Some(hex(&Sha256::digest(&bytes)))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub async fn fetch(hub: &str, token: &str, want: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding this runner")?;
    let bytes = reqwest::Client::new()
        .get(format!("{}/dist/runner", hub.trim_end_matches('/')))
        .bearer_auth(token)
        .timeout(Duration::from_secs(300))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if hex(&Sha256::digest(&bytes)) != want {
        bail!("the downloaded runner isn't the one the hub announced");
    }
    let next = exe.with_extension("next");
    tokio::fs::write(&next, &bytes).await?;
    tokio::fs::set_permissions(&next, std::os::unix::fs::PermissionsExt::from_mode(0o755)).await?;
    let ok = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(&next)
            .arg("--version")
            .output(),
    )
    .await
    .context("the new runner didn't answer")??
    .status
    .success();
    if !ok {
        let _ = tokio::fs::remove_file(&next).await;
        bail!("the new runner doesn't run on this machine");
    }
    Ok(next)
}

pub fn switch(next: PathBuf) -> anyhow::Error {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return e.into(),
    };
    if let Err(e) = std::fs::rename(&next, &exe) {
        return anyhow::anyhow!("couldn't replace {}: {e}", exe.display());
    }
    tracing::info!("restarting into the hub's newer runner");
    std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .exec()
        .into()
}
