use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentSettings, Delta, Event, InputPart, MachineInfo, RequestResponse, StartSpec, ThreadId,
    ThreadStatus,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RunnerMsg {
    Hello {
        protocol: u32,
        info: MachineInfo,
        threads: Vec<RunningThread>,
        #[serde(default)]
        takes: bool,
        #[serde(default)]
        prompt: bool,
    },
    Event {
        thread: ThreadId,
        seq: i64,
        ts: i64,
        event: Event,
    },
    Delta {
        thread: ThreadId,
        turn: String,
        item: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        delta: Delta,
    },
    ThreadStatus {
        thread: ThreadId,
        status: ThreadStatus,
    },
    Status {
        thread: ThreadId,
        status: Value,
    },
    TermOutput {
        term: String,
        data: String,
    },
    TermExit {
        term: String,
    },
    ThreadEnded {
        thread: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Took {
        delivery: String,
        taken: bool,
    },
    Reply {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        err: Option<String>,
    },
    Call {
        id: u64,
        thread: ThreadId,
        #[serde(default)]
        turn: String,
        action: String,
        input: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningThread {
    pub thread: ThreadId,
    pub last_seq: i64,
    pub status: ThreadStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HubMsg {
    Welcome {
        machine: crate::MachineId,
        acks: Vec<(ThreadId, i64)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        git_token: Option<String>,
    },
    Ack {
        thread: ThreadId,
        seq: i64,
    },
    Prompt {
        spec: Box<StartSpec>,
        input: Vec<InputPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settings: Option<AgentSettings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<String>,
    },
    Start {
        spec: Box<StartSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Vec<InputPart>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<String>,
    },
    Send {
        thread: ThreadId,
        input: Vec<InputPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settings: Option<AgentSettings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<String>,
    },
    Interrupt {
        thread: ThreadId,
    },
    StopTask {
        thread: ThreadId,
        tool: String,
    },
    SignIn {
        thread: ThreadId,
        request: String,
        code: String,
    },
    Respond {
        thread: ThreadId,
        request: String,
        response: RequestResponse,
    },
    Settings {
        thread: ThreadId,
        settings: AgentSettings,
    },
    Stop {
        thread: ThreadId,
    },
    TermInput {
        term: String,
        data: String,
    },
    TermResize {
        term: String,
        cols: u16,
        rows: u16,
    },
    TermClose {
        term: String,
    },
    Request {
        id: u64,
        body: RunnerRequest,
    },
    CallResult {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        err: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RunnerRequest {
    ListDir {
        path: String,
    },
    ReadFile {
        path: String,
        max_bytes: u64,
    },
    Git {
        cwd: String,
        args: Vec<String>,
    },
    Prepare {
        cwd: String,
        repos: Vec<crate::Repo>,
        #[serde(default)]
        setup: Option<String>,
        #[serde(default)]
        kit: Option<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
        #[serde(default)]
        force: bool,
    },
    Rewind {
        cwd: String,
        thread: ThreadId,
        turn: String,
        #[serde(default)]
        turns: Vec<String>,
    },
    SaveWork {
        cwd: String,
        thread: ThreadId,
    },
    TermOpen {
        term: String,
        cwd: String,
        cols: u16,
        rows: u16,
    },
    TermAttach {
        term: String,
    },
    Secret {
        name: String,
        value: String,
    },
    Changes {
        cwd: String,
    },
    Folders {
        path: String,
    },
    ClaudeSessions,
    ImportSession {
        session: String,
        #[serde(default)]
        cwd: Option<String>,
    },
    Stop {
        thread: ThreadId,
    },
}
