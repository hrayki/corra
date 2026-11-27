use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use corra_proto::app::HubToApp;
use corra_proto::*;
use serde_json::{Value, json};

use super::{Hub, NewProject, Outbound};

impl Hub {
    pub fn default_provider(&self) -> String {
        if let Some(p) = self.setting_str("default_provider") {
            return p;
        }
        let has = |k: &str| self.secrets.get(k).ok().flatten().is_some();
        if has("modal.token_id") {
            "modal".into()
        } else {
            self.providers.default_name().to_string()
        }
    }

    pub fn create_project(&self, n: NewProject) -> Result<Project> {
        let machine = n.machine;
        let mut repos: Vec<Repo> = vec![];
        for full in &n.repos {
            let full = full.trim().trim_end_matches(".git").to_string();
            let short = full.rsplit('/').next().unwrap_or(&full).to_string();
            if !full.contains('/') || repos.iter().any(|r| r.name == short) {
                continue;
            }
            repos.push(Repo {
                name: short,
                github: Some(full),
            });
        }
        let base = match n.name.trim() {
            "" => repos
                .first()
                .map(|r| r.name.clone())
                .ok_or_else(|| anyhow!("give the project a name or a repo"))?,
            name => name.to_string(),
        };
        if !base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            bail!("a project name may use letters, digits, '-', '_' and '.'");
        }
        let mut name = base.clone();
        let mut i = 2;
        while self.db.project_by_name(&name)?.is_some() {
            name = format!("{base}-{i}");
            i += 1;
        }
        let provider = match machine {
            Some(m) => {
                self.db
                    .machine(m)?
                    .ok_or_else(|| anyhow!("no such machine"))?
                    .provider
            }
            None => n.provider.unwrap_or_else(|| self.default_provider()),
        };
        let mut env: Vec<String> = vec![];
        for e in n.env {
            let e = e.trim().to_string();
            if !e.is_empty() && !env.contains(&e) {
                env.push(e);
            }
        }
        let mut p = Project {
            id: new_id(),
            name,
            repos,
            default_machine: machine,
            provider: provider.clone(),
            kind: None,
            size: "4x8".into(),
            settings: AgentSettings {
                permission_mode: n.permission_mode,
                ..Default::default()
            },
            path: n
                .path
                .map(|p| p.trim().trim_end_matches('/').to_string())
                .filter(|p| !p.is_empty()),
            setup: n.setup.filter(|s| !s.trim().is_empty()),
            env,
            tools: n.tools.unwrap_or_else(default_tools),
            allow_main: n.allow_main,
            trusted: n.trusted,
            created_at: now_ms(),
        };
        self.place_project(&mut p, Some(provider), n.kind, n.size)?;
        self.db.insert_project(&p)?;
        self.publish_project(&p);
        Ok(p)
    }

    pub fn place_project(
        &self,
        p: &mut Project,
        provider: Option<String>,
        kind: Option<String>,
        size: Option<String>,
    ) -> Result<()> {
        let before = (
            p.provider.clone(),
            self.providers.kind_name(&p.provider, p.kind.as_deref()),
        );
        if let Some(pr) = provider {
            if self.providers.get(&pr).is_none() {
                bail!("unknown provider {pr}");
            }
            if pr != p.provider {
                p.provider = pr;
                p.kind = None;
            }
        }
        let kinds = self
            .providers
            .get(&p.provider)
            .map(|x| x.describe().kinds)
            .unwrap_or_default();
        if let Some(k) = kind.filter(|k| !k.is_empty()) {
            if !kinds.iter().any(|x| x.name == k) {
                let names: Vec<_> = kinds.iter().map(|x| x.name.as_str()).collect();
                bail!(
                    "{} has no kind called {k}; it has {}",
                    p.provider,
                    if names.is_empty() {
                        "none".to_string()
                    } else {
                        names.join(", ")
                    }
                );
            }
            p.kind = Some(k);
        }
        if p.kind.is_none() && !kinds.is_empty() && p.default_machine.is_none() {
            p.kind = kinds.first().map(|k| k.name.clone());
        }
        if let Some(sz) = size.filter(|s| !s.trim().is_empty()) {
            p.size = sz.trim().to_string();
        }
        if let Some(k) = self.providers.kind(&p.provider, p.kind.as_deref())
            && !k.sizes.is_empty()
        {
            p.size = crate::providers::fit(&k.sizes, &p.size);
        }
        let after = (
            p.provider.clone(),
            self.providers.kind_name(&p.provider, p.kind.as_deref()),
        );
        if after != before
            && self
                .providers
                .get(&p.provider)
                .is_some_and(|x| x.creates_machines())
        {
            p.default_machine = None;
        }
        Ok(())
    }

    pub fn project_cwd(p: &Project) -> String {
        p.path
            .clone()
            .unwrap_or_else(|| format!("~/projects/{}", p.name))
    }

    pub fn project_stage(&self, project: ProjectId) -> Option<String> {
        self.project_stages.lock().unwrap().get(&project).cloned()
    }

    pub fn set_project_stage(&self, project: ProjectId, stage: Option<String>) {
        {
            let mut stages = self.project_stages.lock().unwrap();
            match &stage {
                Some(s) => {
                    stages.insert(project, s.clone());
                }
                None => {
                    stages.remove(&project);
                }
            }
        }
        self.publish_project_status(project);
    }

    pub fn project_setup_log(&self, project: ProjectId) -> Option<SetupLog> {
        let raw = self
            .db
            .kv_get(&format!("project:{project}:setup_log"))
            .ok()
            .flatten()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn save_project_setup_log(&self, project: ProjectId, log: &SetupLog) {
        if let Ok(s) = serde_json::to_string(log) {
            let _ = self.db.kv_set(&format!("project:{project}:setup_log"), &s);
        }
        self.publish_project_status(project);
    }

    pub fn publish_project_status(&self, project: ProjectId) {
        self.publish(Outbound::All(HubToApp::ProjectStatus {
            project,
            stage: self.project_stage(project),
            setup: self.project_setup_log(project),
        }));
    }

    pub fn project_status_messages(&self) -> Vec<HubToApp> {
        let Ok(projects) = self.db.projects() else {
            return vec![];
        };
        projects
            .into_iter()
            .filter_map(|p| {
                let stage = self.project_stage(p.id);
                let setup = self.project_setup_log(p.id);
                (stage.is_some() || setup.is_some()).then_some(HubToApp::ProjectStatus {
                    project: p.id,
                    stage,
                    setup,
                })
            })
            .collect()
    }

    pub fn project_runtime_info(&self, project: ProjectId) -> Result<Value> {
        let p = self
            .db
            .project(project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        let env_set: Vec<String> = self.project_env(&p).into_iter().map(|(k, _)| k).collect();
        let manager_env: Vec<String> = if p.trusted {
            self.trusted_env().into_iter().map(|(k, _)| k).collect()
        } else {
            vec![]
        };
        let mut tools = vec!["github.*", "threads.*", "secret.request"];
        if p.trusted {
            tools.extend(["projects.*", "machines.*", "credentials.*"]);
        }
        Ok(json!({
            "stage": self.project_stage(project),
            "setup": self.project_setup_log(project),
            "env_injected": env_set,
            "manager_env": manager_env,
            "tools": tools,
        }))
    }

    pub fn remove_repo(&self, project: ProjectId, name: &str) -> Result<Project> {
        let mut p = self
            .db
            .project(project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        let before = p.repos.len();
        p.repos
            .retain(|r| r.name != name && r.github.as_deref() != Some(name));
        if p.repos.len() == before {
            bail!("no such repo {name}");
        }
        self.db.update_project(&p)?;
        self.publish_project(&p);
        Ok(p)
    }

    pub async fn wake_project(self: &Arc<Self>, project: ProjectId) -> Result<MachineId> {
        let p = self
            .db
            .project(project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        let m = self.providers.start_for_project(self, &p).await?;
        self.prepare_paired(p.id, m).await?;
        self.compact_woke.lock().unwrap().remove(&m);
        self.touch_machine(m);
        Ok(m)
    }

    pub async fn sleep_project(self: &Arc<Self>, project: ProjectId) -> Result<()> {
        crate::lifecycle::sleep_project(self, project).await
    }

    pub async fn rerun_setup(self: &Arc<Self>, project: ProjectId) -> Result<SetupLog> {
        let p = self
            .db
            .project(project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        let m = self.providers.start_for_project(self, &p).await?;
        self.compact_woke.lock().unwrap().remove(&m);
        self.touch_machine(m);
        let log = crate::kit::prepare_with(self, &p, m, true).await?;
        if let Some(c) = self.runners.lock().unwrap().get_mut(&m) {
            c.prepared.insert(p.id);
        }
        Ok(log)
    }

    pub async fn delete_project(self: &Arc<Self>, project: ProjectId) -> Result<()> {
        let _ = self
            .db
            .project(project)?
            .ok_or_else(|| anyhow!("no such project"))?;
        let threads = self.db.threads_in_project(project)?;
        for t in &threads {
            self.stop_live(t);
        }
        crate::lifecycle::reset_project(self, project).await?;
        for t in threads {
            self.remove_thread(t.id)?;
        }
        self.project_stages.lock().unwrap().remove(&project);
        self.db.delete_project(project)?;
        self.publish(Outbound::All(HubToApp::ProjectRemoved { project }));
        Ok(())
    }
}
