use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentSettings, Delta, Event, InputPart, MachineInfo, RequestResponse, StartSpec, ThreadId,
    ThreadStatus,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RunnerMsg {
    TunnelOpen {
        id: String,
        port: u16,
    },
    TunnelData {
        id: String,
        data: String,
    },
    TunnelClose {
        id: String,
    },
    Hello {
        protocol: u32,
        info: MachineInfo,
        threads: Vec<RunningThread>,
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
    BrowserFrame {
        data: String,
        width: u32,
        height: u32,
        url: String,
        title: String,
    },
    PhoneVideo {
        width: u32,
        height: u32,
    },
    PhoneFrame {
        data: String,
        config: bool,
        key: bool,
    },
    ThreadEnded {
        thread: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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
    TunnelOpen {
        id: String,
        port: u16,
    },
    TunnelData {
        id: String,
        data: String,
    },
    TunnelClose {
        id: String,
    },
    Welcome {
        machine: crate::MachineId,
        acks: Vec<(ThreadId, i64)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runner: Option<String>,
    },
    Ack {
        thread: ThreadId,
        seq: i64,
    },
    Start {
        spec: Box<StartSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Vec<InputPart>>,
    },
    Send {
        thread: ThreadId,
        input: Vec<InputPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settings: Option<AgentSettings>,
    },
    Interrupt {
        thread: ThreadId,
    },
    StopTask {
        thread: ThreadId,
        tool: String,
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
    BrowserInput {
        method: String,
        params: Value,
    },
    BrowserStop,
    PhoneInput {
        input: crate::PhoneInput,
    },
    PhoneStop,
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
    BrowserOpen {
        width: u32,
        height: u32,
    },
    Phone {
        action: crate::PhoneOp,
    },
    Upload {
        path: String,
        url: String,
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
}
