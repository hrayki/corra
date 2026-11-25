use anyhow::{Result, anyhow};
use corra_proto::*;

use crate::hub::Hub;

const KEY: &str = "leftovers";

pub fn list(hub: &Hub) -> Vec<Leftover> {
    hub.db
        .kv_get(KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(hub: &Hub, all: &[Leftover]) {
    let _ = hub.db.kv_set(
        KEY,
        &serde_json::to_string(all).unwrap_or_else(|_| "[]".into()),
    );
}

pub fn add(
    hub: &Hub,
    what: &str,
    provider: &str,
    thing: LeftoverThing,
    reference: &str,
    error: &str,
) {
    let mut all = list(hub);
    all.retain(|l| !(l.provider == provider && l.reference == reference && l.thing == thing));
    all.push(Leftover {
        id: new_id().simple().to_string(),
        what: what.into(),
        provider: provider.into(),
        thing,
        reference: reference.into(),
        error: error.into(),
        at: now_ms(),
    });
    save(hub, &all);
}

async fn remove(hub: &Hub, provider: &str, thing: LeftoverThing, reference: &str) -> Result<()> {
    let p = hub
        .providers
        .get(provider)
        .ok_or_else(|| anyhow!("this hub has no provider called {provider} any more"))?;
    match thing {
        LeftoverThing::Machine => p.destroy(hub, reference).await,
        LeftoverThing::Image => p.forget(hub, reference).await,
    }
}

pub async fn clean(
    hub: &Hub,
    what: &str,
    provider: &str,
    thing: LeftoverThing,
    reference: &str,
) -> bool {
    match remove(hub, provider, thing, reference).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("couldn't delete {what} on {provider}: {e:#}");
            add(hub, what, provider, thing, reference, &format!("{e:#}"));
            false
        }
    }
}

pub async fn retry(hub: &Hub) -> Vec<Leftover> {
    let mut done = std::collections::HashSet::new();
    let mut failed = std::collections::HashMap::new();
    for l in list(hub) {
        match remove(hub, &l.provider, l.thing, &l.reference).await {
            Ok(()) => {
                done.insert(l.id);
            }
            Err(e) => {
                failed.insert(l.id, format!("{e:#}"));
            }
        }
    }
    let mut all = list(hub);
    all.retain(|l| !done.contains(&l.id));
    for l in &mut all {
        if let Some(e) = failed.remove(&l.id) {
            l.error = e;
            l.at = now_ms();
        }
    }
    save(hub, &all);
    all
}
