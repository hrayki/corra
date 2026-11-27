use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use corra_proto::*;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    repos TEXT NOT NULL DEFAULT '[]',
    default_machine TEXT,
    provider TEXT NOT NULL,
    size TEXT NOT NULL,
    settings TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    path TEXT,
    setup TEXT,
    env TEXT NOT NULL DEFAULT '[]',
    tools TEXT NOT NULL DEFAULT '["browser","phone"]',
    allow_main INTEGER NOT NULL DEFAULT 0,
    trusted INTEGER NOT NULL DEFAULT 0,
    kind TEXT
);
CREATE TABLE IF NOT EXISTS machines (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    status TEXT NOT NULL,
    project TEXT,
    size TEXT NOT NULL,
    info TEXT,
    token_hash TEXT NOT NULL UNIQUE,
    provider_ref TEXT,
    created_at INTEGER NOT NULL,
    last_seen INTEGER,
    install_id TEXT,
    sleep_ref TEXT,
    kind TEXT
);
CREATE TABLE IF NOT EXISTS threads (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    title TEXT NOT NULL,
    agent TEXT NOT NULL,
    machine TEXT,
    session_id TEXT,
    cwd TEXT NOT NULL,
    settings TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL,
    last_seq INTEGER NOT NULL DEFAULT 0,
    runner_seq INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    pinned_at INTEGER,
    settled_at INTEGER,
    hidden_at INTEGER,
    started_by TEXT,
    follower TEXT
);
CREATE INDEX IF NOT EXISTS threads_by_project ON threads(project, updated_at);
CREATE TABLE IF NOT EXISTS events (
    thread TEXT NOT NULL,
    seq INTEGER NOT NULL,
    ts INTEGER NOT NULL,
    kind TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (thread, seq)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS events_by_kind ON events(thread, kind, seq);
CREATE TABLE IF NOT EXISTS kv (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS thread_files (
    thread TEXT NOT NULL,
    path TEXT NOT NULL,
    part INTEGER NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (thread, path, part)
) WITHOUT ROWID;
"#;

pub const SCHEMA_VERSION: i64 = 14;
const OLDEST_VERSION: i64 = 12;
pub const NEWER_DATABASE: i32 = 65;

#[derive(Debug)]
pub struct NewerDatabase(pub i64);

impl std::fmt::Display for NewerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "the database is at schema version {}, which a newer hub made; this one knows up to {SCHEMA_VERSION}, so it won't open it",
            self.0
        )
    }
}

impl std::error::Error for NewerDatabase {}

pub fn is_newer_database(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<NewerDatabase>())
}

pub fn schema_of(path: &Path) -> Result<Option<i64>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(Some(conn.pragma_query_value(
        None,
        "user_version",
        |r| r.get(0),
    )?))
}

fn migrate(conn: &Connection) -> Result<()> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        return Err(NewerDatabase(v).into());
    }
    let fresh: bool = conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'projects')",
        [],
        |r| r.get(0),
    )?;
    if !fresh && v < OLDEST_VERSION {
        anyhow::bail!(
            "this database is from a corra older than its schema version {OLDEST_VERSION} (it's at {v}), which this hub can't open"
        );
    }
    conn.execute_batch(SCHEMA)?;
    if !fresh && v < 13 {
        conn.execute_batch(
            "ALTER TABLE projects ADD COLUMN kind TEXT;
             ALTER TABLE machines ADD COLUMN kind TEXT;",
        )?;
    }
    if !fresh && v < 14 {
        conn.execute_batch(
            "ALTER TABLE threads ADD COLUMN started_by TEXT;
             ALTER TABLE threads ADD COLUMN follower TEXT;
             UPDATE threads SET started_by = (SELECT value FROM kv WHERE kv.key = 'thread:' || threads.id || ':started_by')
               WHERE EXISTS (SELECT 1 FROM kv WHERE kv.key = 'thread:' || threads.id || ':started_by');
             UPDATE threads SET follower = (SELECT value FROM kv WHERE kv.key = 'thread:' || threads.id || ':follower')
               WHERE EXISTS (SELECT 1 FROM kv WHERE kv.key = 'thread:' || threads.id || ':follower');
             DELETE FROM kv WHERE key LIKE 'thread:%:started_by' OR key LIKE 'thread:%:follower';",
        )?;
    }
    conn.execute_batch(
        "UPDATE machines SET provider = 'existing' WHERE provider = 'local';
         UPDATE projects SET provider = 'existing' WHERE provider = 'local';",
    )?;
    if v < SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

fn parse<T: serde::de::DeserializeOwned + Default>(s: Option<String>) -> T {
    s.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn uuid(s: String) -> uuid::Uuid {
    s.parse().unwrap_or_default()
}

fn opt_uuid(s: Option<String>) -> Option<uuid::Uuid> {
    s.and_then(|s| s.parse().ok())
}

pub struct NewMachine {
    pub name: String,
    pub provider: String,
    pub project: Option<ProjectId>,
    pub size: String,
    pub token_hash: String,
    pub provider_ref: Option<String>,
    pub status: MachineStatus,
    pub kind: Option<String>,
}

pub struct MachineAuth {
    pub id: MachineId,
    pub status: MachineStatus,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Db {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap();
        Ok(f(&conn)?)
    }

    pub fn size_bytes(&self) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |r| r.get(0),
            )
        })
    }

    pub fn snapshot_to(&self, path: &Path) -> Result<()> {
        self.with(|c| {
            c.execute("VACUUM INTO ?1", [path.to_string_lossy().to_string()])
                .map(|_| ())
        })
    }

    pub fn file_sizes(&self, thread: ThreadId) -> Result<Vec<(String, u64)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT path, SUM(length(data)) FROM thread_files WHERE thread = ?1 GROUP BY path ORDER BY path",
            )?;
            st.query_map([thread.to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })?
            .collect()
        })
    }

    pub fn file_read(&self, thread: ThreadId, path: &str) -> Result<Option<Vec<u8>>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT data FROM thread_files WHERE thread = ?1 AND path = ?2 ORDER BY part",
            )?;
            let parts: Vec<Vec<u8>> = st
                .query_map(params![thread.to_string(), path], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok((!parts.is_empty()).then(|| parts.concat()))
        })
    }

    pub fn file_replace(&self, thread: ThreadId, path: &str, data: &[u8]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM thread_files WHERE thread = ?1 AND path = ?2",
            params![thread.to_string(), path],
        )?;
        tx.execute(
            "INSERT INTO thread_files(thread, path, part, data) VALUES (?1, ?2, 0, ?3)",
            params![thread.to_string(), path, data],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn file_append(
        &self,
        thread: ThreadId,
        path: &str,
        offset: u64,
        data: &[u8],
    ) -> Result<std::result::Result<u64, u64>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (size, last): (i64, i64) = tx.query_row(
            "SELECT COALESCE(SUM(length(data)), 0), COALESCE(MAX(part), -1) FROM thread_files WHERE thread = ?1 AND path = ?2",
            params![thread.to_string(), path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if size as u64 != offset {
            return Ok(Err(size as u64));
        }
        tx.execute(
            "INSERT INTO thread_files(thread, path, part, data) VALUES (?1, ?2, ?3, ?4)",
            params![thread.to_string(), path, last + 1, data],
        )?;
        tx.commit()?;
        Ok(Ok(offset + data.len() as u64))
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
                .optional()
        })
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO kv(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
        })
    }

    pub fn first_send(&self, id: &str, now: i64) -> Result<bool> {
        self.with(|c| {
            let added = c.execute(
                "INSERT OR IGNORE INTO kv(key, value) VALUES (?1, ?2)",
                params![format!("sent:{id}"), now.to_string()],
            )?;
            Ok(added == 1)
        })
    }

    pub fn forget_sends_before(&self, at: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM kv WHERE key >= 'sent:' AND key < 'sent;' AND CAST(value AS INTEGER) < ?1",
                [at],
            )
            .map(|_| ())
        })
    }

    pub fn kv_keys(&self, prefix: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st =
                c.prepare("SELECT key FROM kv WHERE key >= ?1 AND key < ?2 ORDER BY key")?;
            let end = format!("{prefix}\u{10FFFF}");
            st.query_map(params![prefix, end], |r| r.get(0))?.collect()
        })
    }

    pub fn kv_delete(&self, key: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM kv WHERE key = ?1", [key])
                .map(|_| ())
        })
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, name, repos, default_machine, provider, size, settings, created_at, path, setup, env, tools, allow_main, trusted, kind FROM projects ORDER BY name",
            )?;
            st.query_map([], row_project)?.collect()
        })
    }

    pub fn project(&self, id: ProjectId) -> Result<Option<Project>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, name, repos, default_machine, provider, size, settings, created_at, path, setup, env, tools, allow_main, trusted, kind FROM projects WHERE id = ?1",
                [id.to_string()],
                row_project,
            )
            .optional()
        })
    }

    pub fn insert_project(&self, p: &Project) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO projects(id, name, repos, default_machine, provider, size, settings, created_at, path, setup, env, tools, allow_main, trusted, kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    p.id.to_string(),
                    p.name,
                    json(&p.repos),
                    p.default_machine.map(|m| m.to_string()),
                    p.provider,
                    p.size,
                    json(&p.settings),
                    p.created_at,
                    p.path,
                    p.setup,
                    json(&p.env),
                    json(&p.tools),
                    p.allow_main,
                    p.trusted,
                    p.kind
                ],
            )
            .map(|_| ())
        })
    }

    pub fn update_project(&self, p: &Project) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE projects SET name = ?2, repos = ?3, default_machine = ?4, provider = ?5, size = ?6, settings = ?7, path = ?8, setup = ?9, env = ?10, tools = ?11, allow_main = ?12, trusted = ?13, kind = ?14 WHERE id = ?1",
                params![
                    p.id.to_string(),
                    p.name,
                    json(&p.repos),
                    p.default_machine.map(|m| m.to_string()),
                    p.provider,
                    p.size,
                    json(&p.settings),
                    p.path,
                    p.setup,
                    json(&p.env),
                    json(&p.tools),
                    p.allow_main,
                    p.trusted,
                    p.kind
                ],
            )
            .map(|_| ())
        })
    }

    pub fn project_by_name(&self, name: &str) -> Result<Option<Project>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, name, repos, default_machine, provider, size, settings, created_at, path, setup, env, tools, allow_main, trusted, kind FROM projects WHERE name = ?1",
                [name],
                row_project,
            )
            .optional()
        })
    }

    pub fn delete_project(&self, id: ProjectId) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM kv WHERE key = ?1 OR (key >= ?2 AND key < ?3)",
                params![
                    format!("setup_failed:{id}"),
                    format!("project:{id}:"),
                    format!("project:{id}:\u{10FFFF}"),
                ],
            )?;
            c.execute("DELETE FROM projects WHERE id = ?1", [id.to_string()])
                .map(|_| ())
        })
    }

    pub fn machines(&self) -> Result<Vec<Machine>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, name, provider, status, project, size, info, created_at, last_seen, kind FROM machines WHERE status != 'destroyed' ORDER BY created_at",
            )?;
            st.query_map([], row_machine)?.collect()
        })
    }

    pub fn machine(&self, id: MachineId) -> Result<Option<Machine>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, name, provider, status, project, size, info, created_at, last_seen, kind FROM machines WHERE id = ?1",
                [id.to_string()],
                row_machine,
            )
            .optional()
        })
    }

    pub fn machine_provider_ref(&self, id: MachineId) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT provider_ref FROM machines WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map(|o| o.flatten())
        })
    }

    pub fn insert_machine(&self, m: NewMachine) -> Result<Machine> {
        let id = new_id();
        let now = now_ms();
        self.with(|c| {
            c.execute(
                "INSERT INTO machines(id, name, provider, status, project, size, token_hash, provider_ref, created_at, kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id.to_string(),
                    m.name,
                    m.provider,
                    m.status.as_str(),
                    m.project.map(|p| p.to_string()),
                    m.size,
                    m.token_hash,
                    m.provider_ref,
                    now,
                    m.kind
                ],
            )
            .map(|_| ())
        })?;
        Ok(self.machine(id)?.expect("just inserted"))
    }

    pub fn machine_by_token(&self, token_hash: &str) -> Result<Option<MachineAuth>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, status FROM machines WHERE token_hash = ?1",
                [token_hash],
                |r| {
                    Ok(MachineAuth {
                        id: uuid(r.get(0)?),
                        status: MachineStatus::parse(&r.get::<_, String>(1)?),
                    })
                },
            )
            .optional()
        })
    }

    pub fn machine_auth(&self, id: MachineId) -> Result<Option<(MachineStatus, String)>> {
        self.with(|c| {
            c.query_row(
                "SELECT status, token_hash FROM machines WHERE id = ?1",
                [id.to_string()],
                |r| {
                    Ok((
                        MachineStatus::parse(&r.get::<_, String>(0)?),
                        r.get::<_, String>(1)?,
                    ))
                },
            )
            .optional()
        })
    }

    pub fn set_machine_status(&self, id: MachineId, status: MachineStatus) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE machines SET status = ?2, last_seen = ?3 WHERE id = ?1",
                params![id.to_string(), status.as_str(), now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn set_machine_info(&self, id: MachineId, info: &MachineInfo) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE machines SET info = ?2 WHERE id = ?1",
                params![id.to_string(), json(info)],
            )
            .map(|_| ())
        })
    }

    pub fn set_machine_name(&self, id: MachineId, name: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE machines SET name = ?2 WHERE id = ?1",
                params![id.to_string(), name],
            )
            .map(|_| ())
        })
    }

    pub fn set_machine_token(&self, id: MachineId, token_hash: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE machines SET token_hash = ?2 WHERE id = ?1",
                params![id.to_string(), token_hash],
            )
            .map(|_| ())
        })
    }

    pub fn machine_sleep_ref(&self, id: MachineId) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT sleep_ref FROM machines WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
        })
    }

    pub fn set_machine_sleep_ref(&self, id: MachineId, r: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE machines SET sleep_ref = ?2 WHERE id = ?1",
                params![id.to_string(), r],
            )
            .map(|_| ())
        })
    }

    pub fn set_machine_provider_ref(&self, id: MachineId, r: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE machines SET provider_ref = ?2 WHERE id = ?1",
                params![id.to_string(), r],
            )
            .map(|_| ())
        })
    }

    pub fn threads(&self) -> Result<Vec<Thread>> {
        self.with(|c| {
            let mut st = c.prepare(&format!("{THREAD_SELECT} ORDER BY updated_at DESC"))?;
            st.query_map([], row_thread)?.collect()
        })
    }

    pub fn thread(&self, id: ThreadId) -> Result<Option<Thread>> {
        self.with(|c| {
            c.query_row(
                &format!("{THREAD_SELECT} WHERE id = ?1"),
                [id.to_string()],
                row_thread,
            )
            .optional()
        })
    }

    pub fn threads_on_machine(&self, machine: MachineId) -> Result<Vec<Thread>> {
        self.with(|c| {
            let mut st = c.prepare(&format!("{THREAD_SELECT} WHERE machine = ?1"))?;
            st.query_map([machine.to_string()], row_thread)?.collect()
        })
    }

    pub fn threads_in_project(&self, project: ProjectId) -> Result<Vec<Thread>> {
        self.with(|c| {
            let mut st = c.prepare(&format!("{THREAD_SELECT} WHERE project = ?1"))?;
            st.query_map([project.to_string()], row_thread)?.collect()
        })
    }

    pub fn insert_thread(&self, t: &Thread) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO threads(id, project, title, agent, machine, session_id, cwd, settings, status, last_seq, created_at, updated_at, started_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    t.id.to_string(),
                    t.project.to_string(),
                    t.title,
                    t.agent.as_str(),
                    t.machine.map(|m| m.to_string()),
                    t.session_id,
                    t.cwd,
                    json(&t.settings),
                    t.status.as_str(),
                    t.last_seq,
                    t.created_at,
                    t.updated_at,
                    t.started_by.map(|s| s.to_string()),
                ],
            )
            .map(|_| ())
        })
    }

    pub fn set_started_by(&self, thread: ThreadId, parent: ThreadId) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE threads SET started_by = ?2 WHERE id = ?1",
                params![thread.to_string(), parent.to_string()],
            )
            .map(|_| ())
        })
    }

    pub fn follower(&self, thread: ThreadId) -> Result<Option<ThreadId>> {
        self.with(|c| {
            c.query_row(
                "SELECT follower FROM threads WHERE id = ?1",
                [thread.to_string()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|o| opt_uuid(o.flatten()))
        })
    }

    pub fn set_follower(&self, thread: ThreadId, follower: Option<ThreadId>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE threads SET follower = ?2 WHERE id = ?1",
                params![thread.to_string(), follower.map(|f| f.to_string())],
            )
            .map(|_| ())
        })
    }

    pub fn update_thread(&self, t: &Thread) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE threads SET title = ?2, machine = ?3, session_id = ?4, cwd = ?5, settings = ?6, status = ?7, updated_at = ?8, pinned_at = ?9, settled_at = ?10, hidden_at = ?11 WHERE id = ?1",
                params![
                    t.id.to_string(),
                    t.title,
                    t.machine.map(|m| m.to_string()),
                    t.session_id,
                    t.cwd,
                    json(&t.settings),
                    t.status.as_str(),
                    t.updated_at,
                    t.pinned_at,
                    t.settled_at,
                    t.hidden_at
                ],
            )
            .map(|_| ())
        })
    }

    pub fn runner_seq(&self, thread: ThreadId) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT runner_seq FROM threads WHERE id = ?1",
                [thread.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map(|o| o.unwrap_or(0))
        })
    }

    pub fn reset_runner_seq(&self, thread: ThreadId) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE threads SET runner_seq = 0 WHERE id = ?1",
                [thread.to_string()],
            )
            .map(|_| ())
        })
    }

    pub fn append_event(
        &self,
        thread: ThreadId,
        runner_seq: Option<i64>,
        ts: i64,
        event: &Event,
    ) -> Result<Option<StoredEvent>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (last_seq, last_runner): (i64, i64) = match tx
            .query_row(
                "SELECT last_seq, runner_seq FROM threads WHERE id = ?1",
                [thread.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        {
            Some(v) => v,
            None => return Ok(None),
        };
        if let Some(rs) = runner_seq
            && rs <= last_runner
        {
            return Ok(None);
        }
        let seq = last_seq + 1;
        let kind = serde_json::to_value(event)?
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        tx.execute(
            "INSERT INTO events(thread, seq, ts, kind, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![thread.to_string(), seq, ts, kind, json(event)],
        )?;
        tx.execute(
            "UPDATE threads SET last_seq = ?2, runner_seq = COALESCE(?3, runner_seq), updated_at = ?4 WHERE id = ?1",
            params![thread.to_string(), seq, runner_seq, now_ms()],
        )?;
        tx.commit()?;
        Ok(Some(StoredEvent {
            seq,
            ts,
            event: event.clone(),
        }))
    }

    pub fn upsert_imported_thread(&self, t: &Thread) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO threads(id, project, title, agent, machine, session_id, cwd, settings, status, last_seq, created_at, updated_at, pinned_at, settled_at, hidden_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(id) DO UPDATE SET project = excluded.project, title = excluded.title, machine = excluded.machine,
                   session_id = excluded.session_id, cwd = excluded.cwd, settings = excluded.settings, status = excluded.status,
                   updated_at = excluded.updated_at, pinned_at = excluded.pinned_at, settled_at = excluded.settled_at, hidden_at = excluded.hidden_at",
                params![
                    t.id.to_string(),
                    t.project.to_string(),
                    t.title,
                    t.agent.as_str(),
                    t.machine.map(|m| m.to_string()),
                    t.session_id,
                    t.cwd,
                    json(&t.settings),
                    t.status.as_str(),
                    t.created_at,
                    t.updated_at,
                    t.pinned_at,
                    t.settled_at,
                    t.hidden_at
                ],
            )
            .map(|_| ())
        })
    }

    pub fn insert_imported_events(
        &self,
        thread: ThreadId,
        events: &[StoredEvent],
    ) -> Result<Vec<StoredEvent>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last: i64 = tx
            .query_row(
                "SELECT last_seq FROM threads WHERE id = ?1",
                [thread.to_string()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let mut fresh = vec![];
        let mut max = last;
        for e in events {
            if e.seq != max + 1 {
                continue;
            }
            let kind = serde_json::to_value(&e.event)?
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string();
            tx.execute(
                "INSERT OR IGNORE INTO events(thread, seq, ts, kind, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![thread.to_string(), e.seq, e.ts, kind, json(&e.event)],
            )?;
            max = e.seq;
            fresh.push(e.clone());
        }
        if max > last {
            tx.execute(
                "UPDATE threads SET last_seq = ?2 WHERE id = ?1",
                params![thread.to_string(), max],
            )?;
        }
        tx.commit()?;
        Ok(fresh)
    }

    pub fn delete_events_after(&self, thread: ThreadId, seq: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM events WHERE thread = ?1 AND seq > ?2",
                params![thread.to_string(), seq],
            )?;
            c.execute(
                "UPDATE threads SET last_seq = MIN(last_seq, ?2) WHERE id = ?1",
                params![thread.to_string(), seq],
            )
            .map(|_| ())
        })
    }

    pub fn copy_events(&self, from: ThreadId, to: ThreadId, up_to: i64) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO events(thread, seq, ts, kind, data) SELECT ?2, seq, ts, kind, data FROM events WHERE thread = ?1 AND seq <= ?3",
                params![from.to_string(), to.to_string(), up_to],
            )?;
            let n: i64 = c.query_row("SELECT COALESCE(MAX(seq), 0) FROM events WHERE thread = ?1", [to.to_string()], |r| r.get(0))?;
            c.execute("UPDATE threads SET last_seq = ?2 WHERE id = ?1", params![to.to_string(), n])?;
            Ok(n)
        })
    }

    pub fn delete_thread(&self, thread: ThreadId) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM events WHERE thread = ?1", [thread.to_string()])?;
            c.execute(
                "DELETE FROM thread_files WHERE thread = ?1",
                [thread.to_string()],
            )?;
            c.execute(
                "DELETE FROM kv WHERE key >= ?1 AND key < ?2",
                [
                    format!("thread:{thread}:"),
                    format!("thread:{thread}:\u{10FFFF}"),
                ],
            )?;
            c.execute("DELETE FROM threads WHERE id = ?1", [thread.to_string()])
                .map(|_| ())
        })
    }

    pub fn events_tail(&self, thread: ThreadId, limit: i64) -> Result<Vec<StoredEvent>> {
        let last: i64 = self.with(|c| {
            c.query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM events WHERE thread = ?1",
                [thread.to_string()],
                |r| r.get(0),
            )
        })?;
        self.events_after(thread, (last - limit).max(0), limit)
    }

    pub fn events_after(
        &self,
        thread: ThreadId,
        after: i64,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        self.stored_events(
            "SELECT seq, ts, data FROM events WHERE thread = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
            params![thread.to_string(), after, limit],
        )
    }

    pub fn turn_marks(&self, thread: ThreadId) -> Result<Vec<StoredEvent>> {
        self.stored_events(
            "SELECT seq, ts, data FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind IN ('turn_started', 'turn_completed') ORDER BY seq",
            params![thread.to_string()],
        )
    }

    pub fn turn_starts_before(&self, thread: ThreadId, before: i64, n: i64) -> Result<Vec<i64>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT seq FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = 'turn_started' AND seq < ?2 ORDER BY seq DESC LIMIT ?3",
            )?;
            st.query_map(params![thread.to_string(), before, n], |r| r.get(0))?
                .collect()
        })
    }

    pub fn turns_between(&self, thread: ThreadId, from: i64, before: i64) -> Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = 'turn_started' AND seq >= ?2 AND seq < ?3",
                params![thread.to_string(), from, before],
                |r| r.get(0),
            )
        })
    }

    pub fn turn_start(&self, thread: ThreadId, turn: &str, at_most: i64) -> Result<Option<i64>> {
        self.with(|c| {
            c.query_row(
                "SELECT seq FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = 'turn_started' AND seq <= ?2 AND json_extract(data, '$.turn') = ?3 ORDER BY seq DESC LIMIT 1",
                params![thread.to_string(), at_most, turn],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn event_at(&self, thread: ThreadId, seq: i64) -> Result<Option<StoredEvent>> {
        Ok(self
            .stored_events(
                "SELECT seq, ts, data FROM events WHERE thread = ?1 AND seq = ?2",
                params![thread.to_string(), seq],
            )?
            .pop())
    }

    pub fn messages_between(
        &self,
        thread: ThreadId,
        after: i64,
        upto: i64,
    ) -> Result<Vec<StoredEvent>> {
        self.stored_events(
            "SELECT seq, ts, data FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind IN ('turn_started', 'item_completed') AND seq > ?2 AND seq <= ?3 AND (kind = 'turn_started' OR (json_extract(data, '$.item.type') IN ('user_message', 'agent_message') AND json_extract(data, '$.item.parent') IS NULL)) ORDER BY seq",
            params![thread.to_string(), after, upto],
        )
    }

    pub fn events_between(&self, thread: ThreadId, from: i64, to: i64) -> Result<Vec<StoredEvent>> {
        self.stored_events(
            "SELECT seq, ts, data FROM events WHERE thread = ?1 AND seq >= ?2 AND seq < ?3 ORDER BY seq",
            params![thread.to_string(), from, to],
        )
    }

    pub fn last_of_kind_before(
        &self,
        thread: ThreadId,
        kind: &str,
        before: i64,
    ) -> Result<Option<StoredEvent>> {
        Ok(self
            .stored_events(
                "SELECT seq, ts, data FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = ?2 AND seq < ?3 ORDER BY seq DESC LIMIT 1",
                params![thread.to_string(), kind, before],
            )?
            .pop())
    }

    pub fn last_turns_started(&self, thread: ThreadId, n: i64) -> Result<Vec<StoredEvent>> {
        self.stored_events(
            "SELECT seq, ts, data FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = 'turn_started' ORDER BY seq DESC LIMIT ?2",
            params![thread.to_string(), n],
        )
    }

    pub fn open_requests(&self, thread: ThreadId) -> Result<Vec<StoredEvent>> {
        self.stored_events(
            "SELECT seq, ts, data FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = 'request' AND json_extract(data, '$.id') NOT IN (SELECT json_extract(data, '$.id') FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = 'request_resolved' AND json_extract(data, '$.id') IS NOT NULL) ORDER BY seq",
            params![thread.to_string()],
        )
    }

    pub fn open_turn(&self, thread: ThreadId) -> Result<Option<String>> {
        let Some(Event::TurnStarted { turn, .. }) =
            self.last_turns_started(thread, 1)?.pop().map(|e| e.event)
        else {
            return Ok(None);
        };
        let ended = self.stored_events(
            "SELECT seq, ts, data FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = 'turn_completed' AND seq > (SELECT MAX(seq) FROM events INDEXED BY events_by_kind WHERE thread = ?1 AND kind = 'turn_started') ORDER BY seq",
            params![thread.to_string()],
        )?;
        Ok((!ended
            .iter()
            .any(|e| matches!(&e.event, Event::TurnCompleted { turn: t, .. } if *t == turn)))
        .then_some(turn))
    }

    pub fn last_outcome(&self, thread: ThreadId) -> Result<Option<TurnOutcome>> {
        Ok(
            match self
                .last_of_kind_before(thread, "turn_completed", i64::MAX)?
                .map(|e| e.event)
            {
                Some(Event::TurnCompleted { outcome, .. }) => Some(outcome),
                _ => None,
            },
        )
    }

    fn stored_events(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<StoredEvent>> {
        self.with(|c| {
            let mut st = c.prepare(sql)?;
            st.query_map(p, |r| {
                let data: String = r.get(2)?;
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, data))
            })?
            .map(|row| {
                row.map(|(seq, ts, data)| StoredEvent {
                    seq,
                    ts,
                    event: serde_json::from_str(&data).unwrap_or(Event::Notice {
                        level: NoticeLevel::Warning,
                        text: "unreadable event".into(),
                    }),
                })
            })
            .collect()
        })
    }
}

const THREAD_SELECT: &str = "SELECT id, project, title, agent, machine, session_id, cwd, settings, status, last_seq, created_at, updated_at, pinned_at, settled_at, hidden_at, started_by FROM threads";

fn row_project(r: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: uuid(r.get(0)?),
        name: r.get(1)?,
        repos: parse(r.get(2)?),
        default_machine: opt_uuid(r.get(3)?),
        provider: r.get(4)?,
        size: r.get(5)?,
        settings: parse(r.get(6)?),
        path: r.get(8)?,
        setup: r.get(9)?,
        env: parse(r.get(10)?),
        tools: parse(r.get(11)?),
        allow_main: r.get(12)?,
        trusted: r.get(13)?,
        kind: r.get(14)?,
        created_at: r.get(7)?,
    })
}

fn row_machine(r: &rusqlite::Row) -> rusqlite::Result<Machine> {
    let info: Option<String> = r.get(6)?;
    Ok(Machine {
        id: uuid(r.get(0)?),
        name: r.get(1)?,
        provider: r.get(2)?,
        status: MachineStatus::parse(&r.get::<_, String>(3)?),
        project: opt_uuid(r.get(4)?),
        size: r.get(5)?,
        info: info.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.get(7)?,
        last_seen: r.get(8)?,
        kind: r.get(9)?,
    })
}

fn row_thread(r: &rusqlite::Row) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: uuid(r.get(0)?),
        project: uuid(r.get(1)?),
        title: r.get(2)?,
        agent: AgentKind::parse(&r.get::<_, String>(3)?).unwrap_or(AgentKind::Claude),
        machine: opt_uuid(r.get(4)?),
        session_id: r.get(5)?,
        cwd: r.get(6)?,
        settings: parse(r.get(7)?),
        status: ThreadStatus::parse(&r.get::<_, String>(8)?),
        last_seq: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        pinned_at: r.get(12)?,
        settled_at: r.get(13)?,
        hidden_at: r.get(14)?,
        started_by: opt_uuid(r.get(15)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_a_newer_hub_changed_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corra.db");
        assert_eq!(schema_of(&path).unwrap(), None);
        drop(Db::open(&path).unwrap());
        assert_eq!(schema_of(&path).unwrap(), Some(SCHEMA_VERSION));
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        let err = Db::open(&path).err().unwrap();
        assert!(is_newer_database(&err), "{err:#}");
        assert_eq!(schema_of(&path).unwrap(), Some(SCHEMA_VERSION + 1));
    }
}
