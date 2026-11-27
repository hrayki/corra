pub mod app;
pub mod event;
pub mod runner;

pub use event::*;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub type ThreadId = uuid::Uuid;
pub type ProjectId = uuid::Uuid;
pub type MachineId = uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentKind {
    Claude,
    Antigravity,
}

impl AgentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Antigravity => "antigravity",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(AgentKind::Claude),
            "antigravity" => Some(AgentKind::Antigravity),
            _ => None,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Antigravity => "Antigravity",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Choice {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub efforts: Option<Vec<Choice>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resolved_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub supports_auto_mode: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub permission_mode: Option<String>,
}

impl AgentSettings {
    pub fn merge(&mut self, other: &AgentSettings) {
        if other.model.is_some() {
            self.model = other.model.clone();
        }
        if other.effort.is_some() {
            self.effort = other.effort.clone();
        }
        if other.permission_mode.is_some() {
            self.permission_mode = other.permission_mode.clone();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum InputPart {
    Text { text: String },
    Image { media_type: String, data: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ThreadStatus {
    Idle,
    Starting,
    Running,
    Waiting,
    Error,
}

impl ThreadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThreadStatus::Idle => "idle",
            ThreadStatus::Starting => "starting",
            ThreadStatus::Running => "running",
            ThreadStatus::Waiting => "waiting",
            ThreadStatus::Error => "error",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "starting" => ThreadStatus::Starting,
            "running" => ThreadStatus::Running,
            "waiting" => ThreadStatus::Waiting,
            "error" => ThreadStatus::Error,
            _ => ThreadStatus::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MachineStatus {
    Creating,
    Connected,
    Offline,
    Asleep,
    Destroying,
    Destroyed,
    Failed,
}

impl MachineStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MachineStatus::Creating => "creating",
            MachineStatus::Connected => "connected",
            MachineStatus::Offline => "offline",
            MachineStatus::Asleep => "asleep",
            MachineStatus::Destroying => "destroying",
            MachineStatus::Destroyed => "destroyed",
            MachineStatus::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "creating" => MachineStatus::Creating,
            "connected" => MachineStatus::Connected,
            "destroying" => MachineStatus::Destroying,
            "destroyed" => MachineStatus::Destroyed,
            "failed" => MachineStatus::Failed,
            "asleep" => MachineStatus::Asleep,
            _ => MachineStatus::Offline,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub repos: Vec<Repo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_machine: Option<MachineId>,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<String>,
    pub size: String,
    pub settings: AgentSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub setup: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default = "default_tools")]
    pub tools: Vec<String>,
    #[serde(default)]
    pub allow_main: bool,
    #[serde(default)]
    pub trusted: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Queued {
    pub id: String,
    pub input: Vec<InputPart>,
    pub at: i64,
}

pub fn default_tools() -> Vec<String> {
    vec![]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Repo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub github: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Machine {
    pub id: MachineId,
    pub name: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<String>,
    pub status: MachineStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<ProjectId>,
    pub size: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub info: Option<MachineInfo>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_seen: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MachineInfo {
    pub hostname: String,
    pub os: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub runner_version: String,
    pub home: String,
    #[serde(default)]
    pub agents: Vec<AgentAvailability>,
    #[serde(default)]
    pub disk_used_mb: u64,
    #[serde(default)]
    pub kvm: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentAvailability {
    pub kind: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Thread {
    pub id: ThreadId,
    pub project: ProjectId,
    pub title: String,
    pub agent: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub machine: Option<MachineId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_id: Option<String>,
    pub cwd: String,
    pub settings: AgentSettings,
    pub status: ThreadStatus,
    pub last_seq: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pinned_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub settled_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hidden_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub started_by: Option<ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StartSpec {
    pub thread: ThreadId,
    pub agent: AgentKind,
    pub cwd: String,
    #[serde(default)]
    pub extra_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resume_session: Option<String>,
    pub settings: AgentSettings,
    pub next_seq: i64,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub repos: Vec<Repo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_of: Option<ForkSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForkSource {
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub at_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub thread: Option<ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToolDef {
    pub action: String,
    pub description: String,
    #[ts(type = "unknown")]
    pub input_schema: serde_json::Value,
}

impl ToolDef {
    pub fn mcp_name(&self) -> String {
        self.action.replace('.', "_")
    }
}

pub fn glob_match(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == s;
    }
    let mut rest = s;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(pos) => rest = &rest[pos + part.len()..],
                None => return false,
            }
        }
    }
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CredentialInfo {
    pub name: String,
    pub label: String,
    pub hint: String,
    pub set: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SizeChoice {
    pub cpu: u32,
    pub memory_gb: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KindInfo {
    pub name: String,
    pub label: String,
    pub about: String,
    pub sizes: Vec<SizeChoice>,
    pub sleeps: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_hours: Option<u32>,
    pub kvm: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderInfo {
    pub name: String,
    pub label: String,
    pub about: String,
    pub creates_machines: bool,
    pub credentials: Vec<CredentialInfo>,
    pub kinds: Vec<KindInfo>,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToolInfo {
    pub name: String,
    pub label: String,
    pub about: String,
    pub needs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub host: Option<String>,
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub missing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HubVersion {
    pub version: String,
    pub managed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub latest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub changes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub checked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub check_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub previous: Option<String>,
    #[serde(default)]
    pub trial: bool,
    #[serde(default)]
    pub previous_too_old: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub failed: Option<HubFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub launcher_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HubFailure {
    pub version: String,
    pub reason: String,
    pub at: i64,
    #[serde(default)]
    pub log: Vec<String>,
}
