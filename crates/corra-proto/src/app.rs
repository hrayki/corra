use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{
    AgentKind, AgentSettings, InputPart, LiveDelta, Machine, MachineId, Project, ProjectId,
    RequestResponse, StoredEvent, Thread, ThreadId,
};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppRequest {
    pub id: u64,
    #[serde(flatten)]
    pub body: AppCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
#[ts(export)]
pub enum AppCall {
    Subscribe {
        thread: ThreadId,
        after_seq: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tail: Option<u32>,
    },
    Earlier {
        thread: ThreadId,
        before_seq: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        to_seq: Option<i64>,
    },
    Find {
        thread: ThreadId,
        query: String,
    },
    Unsubscribe {
        thread: ThreadId,
    },
    CreateProject {
        name: String,
        #[serde(default)]
        repos: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        machine: Option<MachineId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        size: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        setup: Option<String>,
        #[serde(default)]
        env: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tools: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        permission_mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        allow_main: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        trusted: Option<bool>,
    },
    AddRepo {
        project: ProjectId,
        github: String,
    },
    ResetMachine {
        project: ProjectId,
    },
    ListRepos,
    ListSetups,
    ListEnv,
    SetEnv {
        name: String,
        value: String,
    },
    DeleteEnv {
        name: String,
    },
    UpdateMachine {
        machine: MachineId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        name: Option<String>,
    },
    MoveThread {
        thread: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        machine: Option<MachineId>,
    },
    ForkThread {
        thread: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        turn: Option<String>,
    },
    RewindThread {
        thread: ThreadId,
        turn: String,
    },
    BackupNow,
    BackupStatus,
    HubVersion {
        #[serde(default)]
        check: bool,
    },
    SwitchHub {
        version: String,
    },
    Sessions,
    RevokeSession {
        session: String,
    },
    GetSettings,
    SetSettings {
        #[ts(type = "Record<string, unknown>")]
        settings: Value,
    },
    CreateThread {
        project: ProjectId,
        agent: AgentKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        machine: Option<MachineId>,
        settings: AgentSettings,
        input: Vec<InputPart>,
    },
    Send {
        thread: ThreadId,
        input: Vec<InputPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        settings: Option<AgentSettings>,
        #[serde(default)]
        queue: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        client_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        compact: Option<bool>,
    },
    DeleteThread {
        thread: ThreadId,
    },
    Unqueue {
        thread: ThreadId,
        id: String,
        #[serde(default)]
        send: bool,
    },
    Interrupt {
        thread: ThreadId,
    },
    StopAgent {
        thread: ThreadId,
        tool: String,
    },
    Respond {
        thread: ThreadId,
        request: String,
        response: RequestResponse,
    },
    AnswerSecret {
        thread: ThreadId,
        request: String,
        value: String,
    },
    UpdateThread {
        thread: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        pinned: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        settled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        hidden: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        settings: Option<AgentSettings>,
    },
    SetSecret {
        name: String,
        value: String,
    },
    DeleteSecret {
        name: String,
    },
    ListSecrets,
    ListProviders,
    ListTools,
    PhoneHost,
    SetPhoneHost {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        host: Option<crate::PhoneHost>,
    },
    Leftovers,
    RetryLeftovers,
    Presence {
        focused: bool,
    },
    PushKey,
    PushSubscribe {
        endpoint: String,
        p256dh: String,
        auth: String,
    },
    PushUnsubscribe,
    PushTest,
    DestroyMachine {
        machine: MachineId,
    },
    UpdateProject {
        project: ProjectId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        repos: Option<Vec<crate::Repo>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        default_machine: Option<MachineId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        setup: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        env: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tools: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        size: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        settings: Option<AgentSettings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        allow_main: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        trusted: Option<bool>,
    },
    ListDir {
        thread: ThreadId,
        path: String,
    },
    Changes {
        thread: ThreadId,
    },
    ListFolders {
        machine: MachineId,
        #[serde(default)]
        path: String,
    },
    ClaudeSessions {
        machine: MachineId,
    },
    ImportSession {
        machine: MachineId,
        session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        project: Option<ProjectId>,
    },
    TermOpen {
        thread: ThreadId,
        cols: u16,
        rows: u16,
    },
    TermAttach {
        thread: ThreadId,
        term: String,
    },
    TermInput {
        thread: ThreadId,
        term: String,
        data: String,
    },
    TermResize {
        thread: ThreadId,
        term: String,
        cols: u16,
        rows: u16,
    },
    TermClose {
        thread: ThreadId,
        term: String,
    },
    BrowserOpen {
        thread: ThreadId,
        width: u32,
        height: u32,
    },
    BrowserInput {
        thread: ThreadId,
        method: String,
        #[ts(type = "Record<string, unknown>")]
        params: Value,
    },
    BrowserClose {
        thread: ThreadId,
    },
    PhoneUp,
    PhoneDown,
    PhoneOpen,
    PhoneInput {
        input: crate::PhoneInput,
    },
    PhoneClose,
    Phone {
        action: crate::PhoneOp,
    },
    ReadFile {
        thread: ThreadId,
        path: String,
    },
    Git {
        thread: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        repo: Option<String>,
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Found {
    pub hits: Vec<FindHit>,
    pub capped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FindHit {
    pub seq: i64,
    pub n: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "t", rename_all = "snake_case")]
#[ts(export)]
pub enum HubToApp {
    Reply {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        ok: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        err: Option<String>,
    },
    Snapshot {
        projects: Vec<Project>,
        machines: Vec<Machine>,
        threads: Vec<Thread>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        build: Option<String>,
    },
    Project {
        project: Project,
    },
    Machine {
        machine: Machine,
    },
    Thread {
        thread: Thread,
    },
    Events {
        thread: ThreadId,
        events: Vec<StoredEvent>,
    },
    Window {
        thread: ThreadId,
        events: Vec<StoredEvent>,
        from_seq: i64,
        earlier: bool,
    },
    Delta {
        delta: LiveDelta,
    },
    Status {
        thread: ThreadId,
        status: Value,
    },
    ThreadRemoved {
        thread: ThreadId,
    },
    Reset {
        thread: ThreadId,
    },
    Queue {
        thread: ThreadId,
        items: Vec<crate::Queued>,
    },
    NotSent {
        thread: ThreadId,
        client_id: String,
    },
    Usage {
        at: i64,
        limits: Value,
    },
    Notify {
        notice: Value,
    },
    TermOutput {
        thread: ThreadId,
        term: String,
        data: String,
    },
    TermExit {
        thread: ThreadId,
        term: String,
    },
    BrowserFrame {
        machine: MachineId,
        data: String,
        width: u32,
        height: u32,
        url: String,
        title: String,
    },
    PhoneVideo {
        machine: MachineId,
        width: u32,
        height: u32,
    },
    PhoneFrame {
        machine: MachineId,
        data: String,
        config: bool,
        key: bool,
    },
}
