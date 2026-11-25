use std::path::PathBuf;

use serde_json::{Map, Value, json};

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".config")
        })
}

fn luminance(hex: &str) -> Option<f64> {
    let h = hex.trim_start_matches('#');
    if h.len() < 6 {
        return None;
    }
    let c = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .ok()
            .map(|v| v as f64 / 255.0)
    };
    Some(0.2126 * c(0)? + 0.7152 * c(2)? + 0.0722 * c(4)?)
}

fn state_home() -> std::path::PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| corra_runner::files::home().join(".local/state"))
}

pub fn omarchy() -> Value {
    let Some((dir, text)) = [
        state_home().join("omarchy/current"),
        config_home().join("omarchy/current"),
    ]
    .into_iter()
    .find_map(|d| {
        std::fs::read_to_string(d.join("theme/colors.toml"))
            .ok()
            .map(|t| (d, t))
    }) else {
        return Value::Null;
    };
    let mut colors = Map::new();
    for line in text.lines() {
        let line = line
            .split('#')
            .next()
            .filter(|l| l.contains('='))
            .map(|_| line)
            .unwrap_or("");
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        let k = k.trim();
        if !k.is_empty() && !v.is_empty() {
            colors.insert(k.to_string(), json!(v));
        }
    }
    let bg = colors
        .get("background")
        .and_then(|v| v.as_str())
        .unwrap_or("#000000")
        .to_string();
    let light = colors.get("mode").and_then(|v| v.as_str()) == Some("light")
        || dir.join("theme/light.mode").exists()
        || luminance(&bg).is_some_and(|l| l > 0.6);
    let name = std::fs::read_to_string(dir.join("theme.name"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    json!({ "name": name, "mode": if light { "light" } else { "dark" }, "colors": colors })
}

pub fn watch() -> tokio::sync::watch::Receiver<Value> {
    let (tx, rx) = tokio::sync::watch::channel(omarchy());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let now = tokio::task::spawn_blocking(omarchy)
                .await
                .unwrap_or(Value::Null);
            tx.send_if_modified(|v| {
                if *v == now {
                    false
                } else {
                    *v = now;
                    true
                }
            });
            if tx.is_closed() {
                return;
            }
        }
    });
    rx
}
