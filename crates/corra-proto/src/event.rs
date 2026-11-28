use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{AgentSettings, InputPart, ThreadId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StoredEvent {
    pub seq: i64,
    pub ts: i64,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Event {
    Session {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        model: Option<String>,
        cwd: String,
        #[serde(default)]
        tools: Vec<String>,
        #[serde(default)]
        slash_commands: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        permission_mode: Option<String>,
        #[serde(default)]
        models: Vec<crate::Choice>,
        #[serde(default)]
        efforts: Vec<crate::Choice>,
        #[serde(default)]
        modes: Vec<crate::Choice>,
    },
    TurnStarted {
        turn: String,
        input: Vec<InputPart>,
        settings: AgentSettings,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        cause: Option<String>,
    },
    ItemStarted {
        turn: String,
        item: Item,
    },
    ItemCompleted {
        turn: String,
        item: Item,
    },
    Request {
        id: String,
        turn: String,
        request: AgentRequest,
    },
    RequestResolved {
        id: String,
        response: RequestResponse,
    },
    TurnCompleted {
        turn: String,
        outcome: TurnOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        anchor: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        resets_at: Option<i64>,
    },
    SettingsChanged {
        settings: AgentSettings,
    },
    Checkpoint {
        turn: String,
        repos: Vec<RepoCheckpoint>,
    },
    Status {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        context: Option<ContextUse>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        rate_limit: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        background: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        agents: Option<Vec<String>>,
    },
    Compacted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        trigger: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        pre_tokens: Option<u64>,
    },
    Notice {
        level: NoticeLevel,
        text: String,
    },
    Unread {
        item: String,
    },
}

impl Event {
    pub fn is_transient(&self) -> bool {
        matches!(self, Event::Status { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoCheckpoint {
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub before: Option<String>,
    pub after: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub branch: Option<String>,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ContextUse {
    pub used_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub window_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Item {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent: Option<String>,
    #[serde(flatten)]
    pub body: ItemBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ItemBody {
    UserMessage {
        content: Vec<InputPart>,
    },
    AgentMessage {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolCall {
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        output: Option<ToolOutput>,
        status: ToolStatus,
    },
    Todos {
        todos: Vec<Todo>,
    },
    TaskResult {
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        summary: Option<String>,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToolOutput {
    pub text: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<InputPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub deferred: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Todo {
    pub content: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub active_form: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum AgentRequest {
    ToolApproval {
        tool: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tool_call: Option<String>,
    },
    Question {
        questions: Vec<Question>,
    },
    PlanApproval {
        plan: String,
    },
    HubAction {
        action: String,
        input: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Question {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub header: Option<String>,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum RequestResponse {
    Allow {
        #[serde(default)]
        always: bool,
    },
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        message: Option<String>,
    },
    Answers {
        answers: Vec<Vec<String>>,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum Delta {
    Text { text: String },
    Reasoning { text: String },
    ToolInput { json: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LiveDelta {
    pub thread: ThreadId,
    pub turn: String,
    pub item: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent: Option<String>,
    pub delta: Delta,
}
