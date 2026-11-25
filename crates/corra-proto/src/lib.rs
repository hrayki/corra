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
}

impl AgentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(AgentKind::Claude),
            _ => None,
        }
    }
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
    vec!["browser".into(), "phone".into()]
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
    #[serde(default = "yes")]
    pub browser: bool,
}

fn yes() -> bool {
    true
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
#[serde(tag = "do", rename_all = "snake_case")]
#[ts(export)]
pub enum PhoneOp {
    Boot,
    Stream,
    Shot,
    Ui,
    Tap {
        x: u32,
        y: u32,
    },
    TapText {
        text: String,
        #[serde(default)]
        index: u32,
    },
    Swipe {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        ms: Option<u32>,
    },
    Text {
        text: String,
    },
    Key {
        key: String,
    },
    Shell {
        command: String,
    },
    Install {
        urls: Vec<String>,
    },
    Forward {
        port: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum PhoneInput {
    Touch {
        action: TouchAction,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
    },
    Scroll {
        x: i32,
        y: i32,
        width: u16,
        height: u16,
        dx: f32,
        dy: f32,
    },
    Text {
        text: String,
    },
    Key {
        key: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TouchAction {
    Down,
    Move,
    Up,
}
