//! Local persistence (§20, §70): SQLite conversations/messages/artifacts.
//! Everything stays on disk; no cloud sync (local-first, §4).

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub model_id: String,
    pub created_at: String,
    /// Stage 14 session mode: "chat" | "code".
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Stage 14: linked workspace id (code sessions) or "" .
    #[serde(default)]
    pub workspace: String,
    /// Stage 11: per-conversation capability defaults (§115).
    #[serde(default)]
    pub reasoning_default: bool,
    #[serde(default)]
    pub search_default: bool,
    /// Stage 17: model that last prepared/generated this thread (§170).
    /// The composer blocks when it differs from the loaded model.
    #[serde(default)]
    pub last_model: String,
    /// Stage 27: scheduling priority — "background" | "normal" | "high".
    #[serde(default = "default_priority")]
    pub priority: String,
    /// Stage 27: optional related session id (§146 metadata link, not shared ctx).
    #[serde(default)]
    pub related_to: String,
}

fn default_priority() -> String {
    "normal".into()
}

fn default_mode() -> String {
    "chat".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String, // user | assistant | tool
    pub content: String,
    pub created_at: String,
}

/// Stage 6: file attached to a conversation (§67). The bytes live on disk
/// under data/attachments/<conv>/; only extracted text is stored in-DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub conversation_id: String,
    pub filename: String,
    pub mime: String,
    pub size_bytes: u64,
    pub text_excerpt: String,
    /// Stage 19: "text" | "image" (§76 status + §73 fallback routing).
    #[serde(default = "default_attach_kind")]
    pub kind: String,
    /// Stage 28: processing state — "ready" | "partial" | "processing" | "unsupported".
    #[serde(default = "default_attach_status")]
    pub status: String,
    pub created_at: String,
}

fn default_attach_status() -> String {
    "ready".into()
}

fn default_attach_kind() -> String {
    "text".into()
}

/// Stage 14: named project boundary (§27, §67).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
    pub build_system: String,
    pub created_at: String,
}

/// Stage 11: record of an explicit web-search request (§121).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRun {
    pub id: String,
    pub conversation_id: String,
    pub query: String,
    pub provider: String,
    pub result_count: usize,
    pub created_at: String,
}

/// Stage 20: generated file record (§38). Bytes live on disk; the row is
/// what the UI renders as an artifact card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub id: String,
    pub conversation_id: String,
    pub filename: String,
    pub path: String,
    pub mime: String,
    pub size_bytes: u64,
    pub created_at: String,
}

/// Stage 26: scoped memory entry (§§82–85). Never bulk-injected into
/// context; the context manager resolves only explicit references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    /// empty = global; otherwise conversation id or workspace id.
    pub scope_id: String,
    /// "session" | "conversation" | "workspace" | "global"
    pub scope: String,
    pub content: String,
    pub source: String,
    pub created_at: String,
    pub last_used: String,
}

#[derive(Debug, Default)]
pub struct MemoryContext {
    pub text: String,
    pub entries: usize,
}

#[derive(Debug, Clone)]
pub struct TaskContext {
    pub conversation_id: String,
    pub workspace: String,
    pub task: String,
    pub run_id: String,
    /// active | planned | interrupted | completed
    pub status: String,
}

/// Stage 35 local knowledge chunk (§39): parsed text kept locally for
/// keyword retrieval. Vector embeddings are staged; scoring is explicit
/// term overlap so results are explainable, never magic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeChunk {
    pub id: String,
    pub workspace_id: String,
    pub path: String,
    pub chunk_idx: i64,
    pub text: String,
}
/// Stage 33 per-message generation metrics (§§32–34): kept separate from
/// message content so telemetry aggregates without duplicating threads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationMetric {
    pub message_id: String,
    pub conversation_id: String,
    pub model_id: String,
    pub prompt_tokens: u32,
    pub generated_tokens: u32,
    pub gen_ms: u64,
    pub ttft_ms: u64,
    pub gen_tps: f64,
    #[serde(default)]
    pub timing: Option<crate::inference::OutputTiming>,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    pub id: String,
    pub conversation_id: String,
    pub tool: String,
    pub args: String,
    pub result: String,
    pub approved: bool,
    pub created_at: String,
}

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn load_settings(&self) -> rusqlite::Result<Option<crate::settings::AppSettings>> {
        let stored = self.conn.query_row(
            "SELECT value FROM settings WHERE key='app_settings'",
            [],
            |row| row.get::<_, String>(0),
        );
        match stored {
            Ok(value) => serde_json::from_str(&value).map(Some).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            }),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn save_settings(&self, settings: &crate::settings::AppSettings) -> rusqlite::Result<()> {
        let value = serde_json::to_string(settings)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.conn.execute("INSERT INTO settings(key,value) VALUES('app_settings',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [value])?;
        Ok(())
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let s = Self { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        let s = Self { conn };
        s.migrate()?;
        s.recover_interrupted_activity()?;
        Ok(s)
    }

    /// Stage 2: file-backed open; creates parent dirs so first launch works.
    pub fn open_persistent(path: &std::path::Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(14),
                        Some(format!("cannot create data dir {}: {e}", parent.display())),
                    )
                })?;
            }
        }
        let conn = Connection::open(path)?;
        // Crash safety over raw speed for conversation history (§83).
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let s = Self { conn };
        s.migrate()?;
        s.recover_interrupted_activity()?;
        Ok(s)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations(
                 id TEXT PRIMARY KEY, title TEXT NOT NULL,
                 model_id TEXT NOT NULL, created_at TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS messages(
                 id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                 role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS tool_executions(
                 id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                 tool TEXT NOT NULL, args TEXT NOT NULL, result TEXT NOT NULL,
                 approved INTEGER NOT NULL DEFAULT 0,
                 created_at TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS artifacts(
                 id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                 filename TEXT NOT NULL, path TEXT NOT NULL,
                 mime TEXT NOT NULL, size_bytes INTEGER NOT NULL DEFAULT 0,
                 created_at TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS attachments(
                 id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                 filename TEXT NOT NULL, mime TEXT NOT NULL,
                 size_bytes INTEGER NOT NULL, text_excerpt TEXT NOT NULL,
                 kind TEXT NOT NULL DEFAULT 'text',
                 created_at TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS settings(
                 key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS workspaces(
                 id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
                 build_system TEXT NOT NULL DEFAULT '',
                 created_at TEXT NOT NULL);
              CREATE TABLE IF NOT EXISTS search_runs(
                  id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                  query TEXT NOT NULL, provider TEXT NOT NULL,
                  result_count INTEGER NOT NULL, created_at TEXT NOT NULL);
              CREATE TABLE IF NOT EXISTS memory_entries(
                  id TEXT PRIMARY KEY, scope_id TEXT NOT NULL DEFAULT '',
                  scope TEXT NOT NULL DEFAULT 'conversation',
                  content TEXT NOT NULL, source TEXT NOT NULL DEFAULT '',
                  created_at TEXT NOT NULL, last_used TEXT NOT NULL);
              CREATE TABLE IF NOT EXISTS knowledge_chunks(
                  id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL DEFAULT '',
                  path TEXT NOT NULL DEFAULT '', chunk_idx INTEGER NOT NULL DEFAULT 0,
                  text TEXT NOT NULL);
              CREATE TABLE IF NOT EXISTS generation_metrics(
                  message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL DEFAULT '',
                  model_id TEXT NOT NULL DEFAULT '', prompt_tokens INTEGER NOT NULL DEFAULT 0,
                  generated_tokens INTEGER NOT NULL DEFAULT 0, gen_ms INTEGER NOT NULL DEFAULT 0,
                  ttft_ms INTEGER NOT NULL DEFAULT 0, gen_tps REAL NOT NULL DEFAULT 0,
                  created_at TEXT NOT NULL DEFAULT '');
              CREATE TABLE IF NOT EXISTS context_summaries(
                  conversation_id TEXT PRIMARY KEY, source_ids TEXT NOT NULL,
                  content TEXT NOT NULL, created_at TEXT NOT NULL);
              CREATE TABLE IF NOT EXISTS message_activities(
                  message_id TEXT NOT NULL, event_json TEXT NOT NULL);
              CREATE INDEX IF NOT EXISTS message_activities_by_message ON message_activities(message_id);
              CREATE TABLE IF NOT EXISTS session_task_context(
                  conversation_id TEXT PRIMARY KEY, workspace TEXT NOT NULL,
                  task TEXT NOT NULL, run_id TEXT NOT NULL, status TEXT NOT NULL);",
        )?;
        // Lightweight forward migration for DBs created before a column existed.
        for (table, column, ddl) in [
            (
                "generation_metrics",
                "timing_json",
                "ALTER TABLE generation_metrics ADD COLUMN timing_json TEXT",
            ),
            (
                "tool_executions",
                "approved",
                "ALTER TABLE tool_executions ADD COLUMN approved INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "conversations",
                "mode",
                "ALTER TABLE conversations ADD COLUMN mode TEXT NOT NULL DEFAULT 'chat'",
            ),
            (
                "conversations",
                "workspace",
                "ALTER TABLE conversations ADD COLUMN workspace TEXT NOT NULL DEFAULT ''",
            ),
            (
                "conversations",
                "reasoning_default",
                "ALTER TABLE conversations ADD COLUMN reasoning_default INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "conversations",
                "search_default",
                "ALTER TABLE conversations ADD COLUMN search_default INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "conversations",
                "last_model",
                "ALTER TABLE conversations ADD COLUMN last_model TEXT NOT NULL DEFAULT ''",
            ),
            (
                "conversations",
                "priority",
                "ALTER TABLE conversations ADD COLUMN priority TEXT NOT NULL DEFAULT 'normal'",
            ),
            (
                "conversations",
                "related_to",
                "ALTER TABLE conversations ADD COLUMN related_to TEXT NOT NULL DEFAULT ''",
            ),
            (
                "attachments",
                "kind",
                "ALTER TABLE attachments ADD COLUMN kind TEXT NOT NULL DEFAULT 'text'",
            ),
            (
                "attachments",
                "status",
                "ALTER TABLE attachments ADD COLUMN status TEXT NOT NULL DEFAULT 'ready'",
            ),
            (
                "artifacts",
                "size_bytes",
                "ALTER TABLE artifacts ADD COLUMN size_bytes INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            let has: bool = self
                .conn
                .prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
                .is_ok();
            if !has {
                self.conn.execute_batch(ddl)?;
            }
        }
        Ok(())
    }

    pub fn create_conversation(&self, c: &Conversation) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO conversations(id,title,model_id,created_at,mode,workspace,reasoning_default,search_default,last_model,priority,related_to) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            params![c.id, c.title, c.model_id, c.created_at, c.mode, c.workspace, c.reasoning_default as i32, c.search_default as i32, c.last_model, c.priority, c.related_to],
        )?;
        Ok(())
    }

    /// Stage 14: update mutable session fields (title, model, mode, workspace, defaults).
    pub fn update_conversation(&self, c: &Conversation) -> rusqlite::Result<bool> {
        let n = self.conn.execute(
            "UPDATE conversations SET title=?,model_id=?,mode=?,workspace=?,reasoning_default=?,search_default=?,last_model=?,priority=?,related_to=? WHERE id=?",
            params![c.title, c.model_id, c.mode, c.workspace, c.reasoning_default as i32, c.search_default as i32, c.last_model, c.priority, c.related_to, c.id],
        )?;
        Ok(n > 0)
    }

    /// Stage 17: record which model prepared/generated a thread (§170).
    pub fn set_last_model(&self, conv: &str, model: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE conversations SET last_model=? WHERE id=?",
            params![model, conv],
        )?;
        Ok(())
    }
    pub fn list_conversations(&self) -> rusqlite::Result<Vec<Conversation>> {
        let mut stmt =
            self.conn.prepare("SELECT id,title,model_id,created_at,mode,workspace,reasoning_default,search_default,last_model,priority,related_to FROM conversations ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(Conversation {
                id: r.get(0)?,
                title: r.get(1)?,
                model_id: r.get(2)?,
                created_at: r.get(3)?,
                mode: r.get(4)?,
                workspace: r.get(5)?,
                reasoning_default: r.get::<_, i32>(6)? != 0,
                search_default: r.get::<_, i32>(7)? != 0,
                last_model: r.get(8)?,
                priority: r.get(9).unwrap_or_else(|_| "normal".into()),
                related_to: r.get(10).unwrap_or_default(),
            })
        })?;
        rows.collect()
    }

    pub fn get_conversation(&self, id: &str) -> rusqlite::Result<Option<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,title,model_id,created_at,mode,workspace,reasoning_default,search_default,last_model,priority,related_to FROM conversations WHERE id=?",
        )?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(Conversation {
                id: r.get(0)?,
                title: r.get(1)?,
                model_id: r.get(2)?,
                created_at: r.get(3)?,
                mode: r.get(4)?,
                workspace: r.get(5)?,
                reasoning_default: r.get::<_, i32>(6)? != 0,
                search_default: r.get::<_, i32>(7)? != 0,
                last_model: r.get(8)?,
                priority: r.get(9).unwrap_or_else(|_| "normal".into()),
                related_to: r.get(10).unwrap_or_default(),
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn delete_conversation(&self, id: &str) -> rusqlite::Result<bool> {
        self.clear_task_context(id)?;
        // Preserve tool history + artifacts on disk; only drop chat rows here.
        // (Artifact GC lands in Phase 2 artifact management.)
        self.conn
            .execute("DELETE FROM messages WHERE conversation_id=?", params![id])?;
        self.conn.execute(
            "DELETE FROM tool_executions WHERE conversation_id=?",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM attachments WHERE conversation_id=?",
            params![id],
        )?;
        let n = self
            .conn
            .execute("DELETE FROM conversations WHERE id=?", params![id])?;
        Ok(n > 0)
    }

    pub fn add_message(&self, m: &Message) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO messages(id,conversation_id,role,content,created_at) VALUES(?,?,?,?,?)",
            params![m.id, m.conversation_id, m.role, m.content, m.created_at],
        )?;
        Ok(())
    }

    pub fn messages_for(&self, conv: &str) -> rusqlite::Result<Vec<Message>> {
        // rowid order = insertion order, stable even when timestamps collide.
        let mut stmt = self.conn.prepare(
            "SELECT id,conversation_id,role,content,created_at FROM messages WHERE conversation_id=? ORDER BY rowid",
        )?;
        let rows = stmt.query_map([conv], |r| {
            Ok(Message {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// A derived inference view. Canonical messages, IDs and timestamps are
    /// never rewritten by compaction, so exports, metrics and forks stay valid.
    pub fn context_messages_for(&self, conv: &str) -> rusqlite::Result<Vec<Message>> {
        let history = self.messages_for(conv)?;
        let mut stmt = self.conn.prepare(
            "SELECT source_ids,content,created_at FROM context_summaries WHERE conversation_id=?",
        )?;
        let summary = stmt.query_row([conv], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        });
        let (encoded_ids, content, created_at) = match summary {
            Ok(summary) => summary,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(history),
            Err(error) => return Err(error),
        };
        let ids: Vec<String> = serde_json::from_str(&encoded_ids).unwrap_or_default();
        // An edit/truncate or old database must never attach a summary to a
        // different prefix. Fall back to canonical history if it no longer fits.
        if ids.is_empty()
            || history.len() < ids.len()
            || !history
                .iter()
                .zip(&ids)
                .all(|(message, id)| message.id == *id)
        {
            return Ok(history);
        }
        let mut view = vec![Message {
            id: format!("context-summary:{conv}"),
            conversation_id: conv.into(),
            role: "assistant".into(),
            content,
            created_at,
        }];
        view.extend(history.into_iter().skip(ids.len()));
        Ok(view)
    }

    pub fn save_context_summary(
        &self,
        conv: &str,
        source_ids: &[String],
        content: &str,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO context_summaries(conversation_id,source_ids,content,created_at) VALUES(?,?,?,?)
             ON CONFLICT(conversation_id) DO UPDATE SET source_ids=excluded.source_ids,content=excluded.content,created_at=excluded.created_at",
            params![conv, serde_json::to_string(source_ids).unwrap_or_default(), content, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn record_message_activity(
        &self,
        mid: &str,
        event: &crate::agent::AgentEvent,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO message_activities(message_id,event_json) VALUES(?,?)",
            params![mid, serde_json::to_string(event).unwrap_or_default()],
        )?;
        Ok(())
    }

    pub fn message_activities(&self, mid: &str) -> rusqlite::Result<Vec<crate::agent::AgentEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_json FROM message_activities WHERE message_id=? ORDER BY rowid",
        )?;
        let rows = stmt.query_map([mid], |row| row.get::<_, String>(0))?;
        let mut events = vec![];
        for row in rows {
            if let Ok(event) = serde_json::from_str(&row?) {
                events.push(event);
            }
        }
        Ok(events)
    }

    pub fn save_task_context(&self, context: &TaskContext) -> rusqlite::Result<()> {
        self.conn.execute("INSERT INTO session_task_context(conversation_id,workspace,task,run_id,status) VALUES(?,?,?,?,?)
            ON CONFLICT(conversation_id) DO UPDATE SET workspace=excluded.workspace,task=excluded.task,run_id=excluded.run_id,status=excluded.status",
            params![context.conversation_id, context.workspace, context.task, context.run_id, context.status])?;
        Ok(())
    }

    pub fn task_context(&self, conversation_id: &str) -> rusqlite::Result<Option<TaskContext>> {
        let context = self.conn.query_row("SELECT conversation_id,workspace,task,run_id,status FROM session_task_context WHERE conversation_id=?", [conversation_id], |row| Ok(TaskContext {
            conversation_id: row.get(0)?, workspace: row.get(1)?, task: row.get(2)?, run_id: row.get(3)?, status: row.get(4)?,
        }));
        match context {
            Ok(context) => Ok(Some(context)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn finish_task_context(
        &self,
        conversation_id: &str,
        run_id: &str,
        status: &str,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE session_task_context SET status=? WHERE conversation_id=? AND run_id=?",
            params![status, conversation_id, run_id],
        )?;
        Ok(())
    }

    pub fn clear_task_context(&self, conversation_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM session_task_context WHERE conversation_id=?",
            [conversation_id],
        )?;
        Ok(())
    }

    /// Reopening a database does not resume an old process. Mark unfinished
    /// journals honestly instead of leaving historical actions spinning forever.
    fn recover_interrupted_activity(&self) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE session_task_context SET status='interrupted' WHERE status='active'",
            [],
        )?;
        let mut statement = self.conn.prepare(
            "SELECT m.id,m.conversation_id,m.content,a.event_json FROM messages m
             JOIN message_activities a ON a.rowid=(SELECT MAX(rowid) FROM message_activities WHERE message_id=m.id)",
        )?;
        let records = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for (id, conversation, content, json) in records {
            let Ok(last) = serde_json::from_str::<crate::agent::AgentEvent>(&json) else {
                continue;
            };
            if matches!(
                last.state,
                crate::agent::AgentState::Completed
                    | crate::agent::AgentState::Failed
                    | crate::agent::AgentState::Cancelled
            ) {
                continue;
            }
            let message = "Interrupted when the runtime closed. Recorded actions and existing files were preserved. Review the partial work before starting again.";
            self.record_message_activity(
                &id,
                &crate::agent::AgentEvent::activity(
                    "status",
                    crate::agent::AgentState::Cancelled,
                    message.into(),
                    last.iteration,
                ),
            )?;
            if content == "Work is starting…" {
                self.update_message_content(&conversation, &id, message)?;
            }
        }
        Ok(())
    }

    pub fn get_message(&self, conv: &str, mid: &str) -> rusqlite::Result<Option<Message>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,conversation_id,role,content,created_at FROM messages WHERE conversation_id=? AND id=?",
        )?;
        let mut rows = stmt.query_map(params![conv, mid], |r| {
            Ok(Message {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn update_message_content(
        &self,
        conv: &str,
        mid: &str,
        content: &str,
    ) -> rusqlite::Result<bool> {
        self.conn.execute(
            "DELETE FROM context_summaries WHERE conversation_id=?",
            [conv],
        )?;
        let n = self.conn.execute(
            "UPDATE messages SET content=? WHERE conversation_id=? AND id=?",
            params![content, conv, mid],
        )?;
        Ok(n > 0)
    }

    /// Delete every message inserted after `mid` (rowid order). Returns the count.
    /// Used by edit-with-truncate: editing an old turn restarts the thread there.
    pub fn delete_messages_after(&self, conv: &str, mid: &str) -> rusqlite::Result<usize> {
        self.clear_task_context(conv)?;
        let n = self.conn.execute(
            "DELETE FROM messages WHERE conversation_id=? AND rowid > (SELECT rowid FROM messages WHERE conversation_id=? AND id=?)",
            params![conv, conv, mid],
        )?;
        Ok(n as usize)
    }

    /// Delete an explicit id set (compaction). Returns the count.
    pub fn delete_messages(&self, conv: &str, ids: &[String]) -> rusqlite::Result<usize> {
        let mut n = 0;
        for id in ids {
            n += self.conn.execute(
                "DELETE FROM messages WHERE conversation_id=? AND id=?",
                params![conv, id],
            )?;
        }
        Ok(n as usize)
    }

    pub fn add_attachment(&self, a: &Attachment) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO attachments(id,conversation_id,filename,mime,size_bytes,text_excerpt,kind,status,created_at) VALUES(?,?,?,?,?,?,?,?,?)",
            params![a.id, a.conversation_id, a.filename, a.mime, a.size_bytes as i64, a.text_excerpt, a.kind, a.status, a.created_at],
        )?;
        Ok(())
    }

    pub fn attachments_for(&self, conv: &str) -> rusqlite::Result<Vec<Attachment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,conversation_id,filename,mime,size_bytes,text_excerpt,kind,status,created_at FROM attachments WHERE conversation_id=? ORDER BY rowid",
        )?;
        let rows = stmt.query_map([conv], |r| {
            Ok(Attachment {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                filename: r.get(2)?,
                mime: r.get(3)?,
                size_bytes: r.get::<_, i64>(4)? as u64,
                text_excerpt: r.get(5)?,
                kind: r.get(6)?,
                status: r.get(7).unwrap_or_else(|_| "ready".into()),
                created_at: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_attachment(&self, conv: &str, aid: &str) -> rusqlite::Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM attachments WHERE conversation_id=? AND id=?",
            params![conv, aid],
        )? > 0)
    }

    pub fn record_tool_execution(&self, t: &ToolExecution) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO tool_executions(id,conversation_id,tool,args,result,approved,created_at) VALUES(?,?,?,?,?,?,?)",
            params![t.id, t.conversation_id, t.tool, t.args, t.result, t.approved as i32, t.created_at],
        )?;
        Ok(())
    }

    pub fn tool_executions_for(
        &self,
        conv: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<ToolExecution>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,conversation_id,tool,args,result,approved,created_at FROM tool_executions WHERE conversation_id=? ORDER BY rowid DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![conv, limit as i64], |r| {
            Ok(ToolExecution {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                tool: r.get(2)?,
                args: r.get(3)?,
                result: r.get(4)?,
                approved: r.get::<_, i32>(5)? != 0,
                created_at: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    // ---- Stage 20 artifacts ----

    pub fn record_artifact(&self, a: &ArtifactRow) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO artifacts(id,conversation_id,filename,path,mime,size_bytes,created_at) VALUES(?,?,?,?,?,?,?)",
            params![a.id, a.conversation_id, a.filename, a.path, a.mime, a.size_bytes as i64, a.created_at],
        )?;
        Ok(())
    }

    pub fn artifacts_for(&self, conv: &str) -> rusqlite::Result<Vec<ArtifactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,conversation_id,filename,path,mime,size_bytes,created_at FROM artifacts WHERE conversation_id=? ORDER BY rowid",
        )?;
        let rows = stmt.query_map([conv], |r| {
            Ok(ArtifactRow {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                filename: r.get(2)?,
                path: r.get(3)?,
                mime: r.get(4)?,
                size_bytes: r.get::<_, i64>(5)? as u64,
                created_at: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_artifact(&self, aid: &str) -> rusqlite::Result<Option<ArtifactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,conversation_id,filename,path,mime,size_bytes,created_at FROM artifacts WHERE id=?",
        )?;
        let mut rows = stmt.query_map([aid], |r| {
            Ok(ArtifactRow {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                filename: r.get(2)?,
                path: r.get(3)?,
                mime: r.get(4)?,
                size_bytes: r.get::<_, i64>(5)? as u64,
                created_at: r.get(6)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    // ---- Stage 14 workspaces ----

    pub fn create_workspace(&self, w: &Workspace) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO workspaces(id,name,path,build_system,created_at) VALUES(?,?,?,?,?)",
            params![w.id, w.name, w.path, w.build_system, w.created_at],
        )?;
        Ok(())
    }

    pub fn list_workspaces(&self) -> rusqlite::Result<Vec<Workspace>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,name,path,build_system,created_at FROM workspaces ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Workspace {
                id: r.get(0)?,
                name: r.get(1)?,
                path: r.get(2)?,
                build_system: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_workspace(&self, id: &str) -> rusqlite::Result<Option<Workspace>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,path,build_system,created_at FROM workspaces WHERE id=?")?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(Workspace {
                id: r.get(0)?,
                name: r.get(1)?,
                path: r.get(2)?,
                build_system: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn delete_workspace(&self, id: &str) -> rusqlite::Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM workspaces WHERE id=?", params![id])?
            > 0)
    }

    // ---- Stage 11 search audit ----

    pub fn record_search_run(&self, run: &SearchRun) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO search_runs(id,conversation_id,query,provider,result_count,created_at) VALUES(?,?,?,?,?,?)",
            params![run.id, run.conversation_id, run.query, run.provider, run.result_count as i64, run.created_at],
        )?;
        Ok(())
    }

    // ---- Stage 26 memory entries ----

    pub fn add_memory(&self, m: &MemoryEntry) -> rusqlite::Result<()> {
        if !["session", "conversation", "workspace", "global"].contains(&m.scope.as_str()) {
            return Err(rusqlite::Error::InvalidParameterName(
                "bad memory scope".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO memory_entries(id,scope_id,scope,content,source,created_at,last_used) VALUES(?,?,?,?,?,?,?)",
            params![m.id, m.scope_id, m.scope, m.content, m.source, m.created_at, m.last_used],
        )?;
        Ok(())
    }

    /// Memories visible to a session: its own conversation id, its workspace
    /// id, and globals. Never the whole store (§82: no silent bulk dump).
    pub fn memories_for(
        &self,
        conv_id: &str,
        workspace_id: &str,
    ) -> rusqlite::Result<Vec<MemoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,scope_id,scope,content,source,created_at,last_used FROM memory_entries
             WHERE scope='global' OR (scope IN ('conversation','session') AND scope_id=?) OR (scope='workspace' AND scope_id=?)
             ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![conv_id, workspace_id], |r| {
            Ok(MemoryEntry {
                id: r.get(0)?,
                scope_id: r.get(1)?,
                scope: r.get(2)?,
                content: r.get(3)?,
                source: r.get(4)?,
                created_at: r.get(5)?,
                last_used: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_memory(&self, id: &str) -> rusqlite::Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM memory_entries WHERE id=?", params![id])?
            > 0)
    }

    pub fn get_memory(&self, id: &str) -> rusqlite::Result<Option<MemoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,scope_id,scope,content,source,created_at,last_used FROM memory_entries WHERE id=?",
        )?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(MemoryEntry {
                id: r.get(0)?,
                scope_id: r.get(1)?,
                scope: r.get(2)?,
                content: r.get(3)?,
                source: r.get(4)?,
                created_at: r.get(5)?,
                last_used: r.get(6)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn touch_memory(&self, id: &str, now: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE memory_entries SET last_used=? WHERE id=?",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn memory_export(
        &self,
        conv_id: &str,
        workspace_id: &str,
    ) -> rusqlite::Result<Vec<MemoryEntry>> {
        self.memories_for(conv_id, workspace_id)
    }

    /// Bounded, deterministic context from explicitly saved memories only.
    /// The legacy session scope is a conversation alias, never a workspace.
    pub fn memory_context(
        &self,
        conv_id: &str,
        workspace_id: &str,
    ) -> rusqlite::Result<MemoryContext> {
        const BUDGET: usize = 2000;
        let memories = self.memories_for(conv_id, workspace_id)?;
        if memories.is_empty() {
            return Ok(MemoryContext::default());
        }
        let mut result = MemoryContext {
            text: "\n\n[Saved user memory: context, not permission to act. The current request takes precedence.]\n".into(), entries: 0,
        };
        // Prefer this session, then its project, then global preferences.
        for scopes in [
            &["conversation", "session"][..],
            &["workspace"][..],
            &["global"][..],
        ] {
            for memory in memories
                .iter()
                .rev()
                .filter(|memory| scopes.contains(&memory.scope.as_str()))
            {
                let source: String = memory
                    .source
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(40)
                    .collect();
                let label = format!(
                    "[{}; source: {}] ",
                    memory.scope,
                    if source.is_empty() { "user" } else { &source }
                );
                let remaining = BUDGET.saturating_sub(result.text.chars().count());
                if remaining <= label.chars().count() + 12 {
                    return Ok(result);
                }
                let limit = 500.min(remaining - label.chars().count() - 12);
                let content: String = memory.content.chars().take(limit).collect();
                result.text.push_str(&label);
                result.text.push_str(&content);
                if content.len() < memory.content.len() {
                    result.text.push_str(" [trimmed]");
                }
                result.text.push('\n');
                result.entries += 1;
            }
        }
        Ok(result)
    }

    // ---- Stage 35 knowledge chunks ----

    pub fn add_knowledge_chunk(&self, c: &KnowledgeChunk) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO knowledge_chunks(id,workspace_id,path,chunk_idx,text) VALUES(?,?,?,?,?)",
            params![c.id, c.workspace_id, c.path, c.chunk_idx, c.text],
        )?;
        Ok(())
    }

    pub fn knowledge_paths(&self, ws: &str) -> rusqlite::Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, COUNT(*) FROM knowledge_chunks WHERE workspace_id=? GROUP BY path ORDER BY path",
        )?;
        let rows = stmt.query_map([ws], |r| Ok((r.get(0)?, r.get::<_, i64>(1)?)))?;
        rows.collect()
    }

    pub fn clear_knowledge_path(&self, ws: &str, path: &str) -> rusqlite::Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM knowledge_chunks WHERE workspace_id=? AND path=?",
            params![ws, path],
        )? as usize)
    }

    pub fn knowledge_for(&self, ws: &str) -> rusqlite::Result<Vec<KnowledgeChunk>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,workspace_id,path,chunk_idx,text FROM knowledge_chunks WHERE workspace_id=? ORDER BY path,chunk_idx",
        )?;
        let rows = stmt.query_map([ws], |r| {
            Ok(KnowledgeChunk {
                id: r.get(0)?,
                workspace_id: r.get(1)?,
                path: r.get(2)?,
                chunk_idx: r.get(3)?,
                text: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    // ---- Stage 33 generation metrics ----

    pub fn record_metric(&self, m: &GenerationMetric) -> rusqlite::Result<()> {
        let timing_json = m
            .timing
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.conn.execute(
            "INSERT OR REPLACE INTO generation_metrics(message_id,conversation_id,model_id,prompt_tokens,generated_tokens,gen_ms,ttft_ms,gen_tps,created_at,timing_json) VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![m.message_id, m.conversation_id, m.model_id, m.prompt_tokens as i64, m.generated_tokens as i64, m.gen_ms as i64, m.ttft_ms as i64, m.gen_tps, m.created_at, timing_json],
        )?;
        Ok(())
    }

    pub fn metrics_for(&self, conv: &str) -> rusqlite::Result<Vec<GenerationMetric>> {
        let mut stmt = self.conn.prepare(
            "SELECT message_id,conversation_id,model_id,prompt_tokens,generated_tokens,gen_ms,ttft_ms,gen_tps,created_at,timing_json FROM generation_metrics WHERE conversation_id=? ORDER BY rowid",
        )?;
        let rows = stmt.query_map([conv], |r| {
            Ok(GenerationMetric {
                message_id: r.get(0)?,
                conversation_id: r.get(1)?,
                model_id: r.get(2)?,
                prompt_tokens: r.get::<_, i64>(3)? as u32,
                generated_tokens: r.get::<_, i64>(4)? as u32,
                gen_ms: r.get::<_, i64>(5)? as u64,
                ttft_ms: r.get::<_, i64>(6)? as u64,
                gen_tps: r.get(7)?,
                created_at: r.get(8)?,
                timing: r
                    .get::<_, Option<String>>(9)?
                    .map(|json| {
                        serde_json::from_str(&json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                9,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })
                    })
                    .transpose()?,
            })
        })?;
        rows.collect()
    }

    // ---- Stage 14 fork: copy a thread into a new conversation ----

    /// Copy all messages (+attachment rows) from `src` to `dst`.
    /// Returns the copied message count. File bytes are shared on disk.
    pub fn fork_messages(&self, src: &str, dst: &str, now: &str) -> rusqlite::Result<usize> {
        let msgs = self.messages_for(src)?;
        let mut n = 0;
        for m in msgs {
            let new_id = uuid::Uuid::new_v4().to_string();
            let activities = self.message_activities(&m.id)?;
            self.add_message(&Message {
                id: new_id.clone(),
                conversation_id: dst.into(),
                role: m.role,
                content: m.content,
                created_at: now.into(),
            })?;
            for event in activities {
                self.record_message_activity(&new_id, &event)?;
            }
            n += 1;
        }
        for a in self.attachments_for(src)? {
            self.add_attachment(&Attachment {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: dst.into(),
                ..a
            })?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_continuation_is_durable_and_old_runs_cannot_overwrite_newer_goals() {
        let root =
            std::env::temp_dir().join(format!("companion-task-context-{}", uuid::Uuid::new_v4()));
        let database = root.join("state.db");
        {
            let st = Storage::open_persistent(&database).unwrap();
            let mut context = TaskContext {
                conversation_id: "session".into(),
                workspace: "project".into(),
                task: "Fix the calculator".into(),
                run_id: "first".into(),
                status: "active".into(),
            };
            st.save_task_context(&context).unwrap();
            context.run_id = "newer".into();
            context.task = "Inspect the parser".into();
            st.save_task_context(&context).unwrap();
            st.finish_task_context("session", "first", "completed")
                .unwrap();
            assert_eq!(
                st.task_context("session").unwrap().unwrap().status,
                "active"
            );
        }
        let st = Storage::open_persistent(&database).unwrap();
        let saved = st.task_context("session").unwrap().unwrap();
        assert_eq!(saved.run_id, "newer");
        assert_eq!(saved.task, "Inspect the parser");
        assert_eq!(saved.status, "interrupted");
        st.finish_task_context("session", "newer", "planned")
            .unwrap();
        assert_eq!(
            st.task_context("session").unwrap().unwrap().status,
            "planned"
        );
        st.delete_conversation("session").unwrap();
        assert!(st.task_context("session").unwrap().is_none());
    }

    #[test]
    fn compaction_is_a_derived_view_and_preserves_message_identity() {
        let storage = Storage::open_in_memory().unwrap();
        for i in 0..20 {
            storage
                .add_message(&Message {
                    id: format!("m{i}"),
                    conversation_id: "context-test".into(),
                    role: if i % 2 == 0 {
                        "user".into()
                    } else {
                        "assistant".into()
                    },
                    content: format!("Original message {i}"),
                    created_at: format!("2026-01-01T00:00:{i:02}Z"),
                })
                .unwrap();
        }
        let original = serde_json::to_value(storage.messages_for("context-test").unwrap()).unwrap();
        let ids = (0..10).map(|i| format!("m{i}")).collect::<Vec<_>>();
        storage
            .save_context_summary("context-test", &ids, "A derived summary")
            .unwrap();
        let context = storage.context_messages_for("context-test").unwrap();
        assert_eq!(context.len(), 11);
        assert_eq!(context[0].content, "A derived summary");
        assert_eq!(context[1].id, "m10");
        assert_eq!(
            serde_json::to_value(storage.messages_for("context-test").unwrap()).unwrap(),
            original
        );
        storage
            .update_message_content("context-test", "m0", "Edited source")
            .unwrap();
        assert_eq!(
            storage.context_messages_for("context-test").unwrap().len(),
            20,
            "editing invalidates stale derived context"
        );
    }

    #[test]
    fn activity_journal_keeps_verified_results_in_order() {
        let storage = Storage::open_in_memory().unwrap();
        let started = crate::agent::AgentEvent::activity(
            "tool_started",
            crate::agent::AgentState::ExecutingTool,
            "Reading".into(),
            1,
        );
        let finished = crate::agent::AgentEvent::activity(
            "tool_result",
            crate::agent::AgentState::Observing,
            "Read complete".into(),
            1,
        );
        storage.record_message_activity("reply", &started).unwrap();
        storage.record_message_activity("reply", &finished).unwrap();
        let events = storage.message_activities("reply").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "tool_started");
        assert_eq!(events[1].kind, "tool_result");
        assert!(storage
            .message_activities("other-reply")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn restart_marks_unfinished_journal_interrupted_without_losing_partial_text() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .add_message(&Message {
                id: "interrupted".into(),
                conversation_id: "session".into(),
                role: "assistant".into(),
                content: "Partial response".into(),
                created_at: "original timestamp".into(),
            })
            .unwrap();
        storage
            .record_message_activity(
                "interrupted",
                &crate::agent::AgentEvent::activity(
                    "tool_started",
                    crate::agent::AgentState::ExecutingTool,
                    "Reading".into(),
                    2,
                ),
            )
            .unwrap();
        storage.recover_interrupted_activity().unwrap();
        let events = storage.message_activities("interrupted").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].state, crate::agent::AgentState::Cancelled);
        assert_eq!(
            storage
                .get_message("session", "interrupted")
                .unwrap()
                .unwrap()
                .content,
            "Partial response"
        );
        storage.recover_interrupted_activity().unwrap();
        assert_eq!(
            storage.message_activities("interrupted").unwrap().len(),
            2,
            "restart recovery is idempotent"
        );
    }

    #[test]
    fn conversation_roundtrip_survives_restart_semantics() {
        let s = Storage::open_in_memory().unwrap();
        s.create_conversation(&Conversation {
            id: "c1".into(),
            title: "t".into(),
            model_id: "m".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            mode: "chat".into(),
            workspace: "".into(),
            reasoning_default: false,
            search_default: false,
            last_model: "".into(),
            priority: "normal".into(),
            related_to: "".into(),
        })
        .unwrap();
        s.add_message(&Message {
            id: "m1".into(),
            conversation_id: "c1".into(),
            role: "user".into(),
            content: "hi".into(),
            created_at: "2026-01-01T00:00:01Z".into(),
        })
        .unwrap();
        assert_eq!(s.list_conversations().unwrap().len(), 1);
        assert_eq!(s.messages_for("c1").unwrap().len(), 1);
    }

    #[test]
    fn persistent_file_survives_reopen_and_delete_cascades_messages() {
        let dir = std::env::temp_dir().join(format!("companion-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = dir.join("companion.db");
        {
            let s = Storage::open_persistent(&db).unwrap();
            s.create_conversation(&Conversation {
                id: "c9".into(),
                title: "t".into(),
                model_id: "m".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                mode: "chat".into(),
                workspace: "".into(),
                reasoning_default: false,
                search_default: false,
                last_model: "".into(),
                priority: "normal".into(),
                related_to: "".into(),
            })
            .unwrap();
            s.add_message(&Message {
                id: "m9".into(),
                conversation_id: "c9".into(),
                role: "user".into(),
                content: "hi".into(),
                created_at: "2026-01-01T00:00:01Z".into(),
            })
            .unwrap();
            assert!(s.get_conversation("c9").unwrap().is_some());
        }
        {
            let s = Storage::open_persistent(&db).unwrap();
            assert_eq!(s.messages_for("c9").unwrap().len(), 1);
            assert!(s.delete_conversation("c9").unwrap());
            assert!(s.get_conversation("c9").unwrap().is_none());
            assert!(s.messages_for("c9").unwrap().is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn seed_thread(s: &Storage) {
        s.create_conversation(&Conversation {
            id: "ce".into(),
            title: "t".into(),
            model_id: "m".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            mode: "chat".into(),
            workspace: "".into(),
            reasoning_default: false,
            search_default: false,
            last_model: "".into(),
            priority: "normal".into(),
            related_to: "".into(),
        })
        .unwrap();
        for (i, role) in ["user", "assistant", "user", "assistant"]
            .iter()
            .enumerate()
        {
            s.add_message(&Message {
                id: format!("e{i}"),
                conversation_id: "ce".into(),
                role: role.to_string(),
                content: format!("msg{i}"),
                created_at: format!("2026-01-01T00:00:0{i}Z"),
            })
            .unwrap();
        }
    }

    #[test]
    fn memory_scopes_are_visible_selectively() {
        let s = Storage::open_in_memory().unwrap();
        let now = "2026-01-01T00:00:00Z";
        for (id, scope, scope_id) in [
            ("g1", "global", ""),
            ("c1", "conversation", "ce"),
            ("w1", "workspace", "ws1"),
            ("o1", "conversation", "other"),
        ] {
            s.add_memory(&MemoryEntry {
                id: id.into(),
                scope_id: scope_id.into(),
                scope: scope.into(),
                content: format!("mem {id}"),
                source: "user".into(),
                created_at: now.into(),
                last_used: now.into(),
            })
            .unwrap();
        }
        let vis: Vec<String> = s
            .memories_for("ce", "ws1")
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert!(vis.contains(&"g1".to_string()));
        assert!(vis.contains(&"c1".to_string()));
        assert!(vis.contains(&"w1".to_string()));
        assert!(
            !vis.contains(&"o1".to_string()),
            "other conversations must not leak: {vis:?}"
        );
        assert!(s.delete_memory("g1").unwrap());
        assert!(!s.delete_memory("g1").unwrap());
    }

    #[test]
    fn prompt_memory_is_scoped_bounded_and_labels_provenance() {
        let storage = Storage::open_in_memory().unwrap();
        for (id, scope, scope_id) in [
            ("own", "conversation", "session-a"),
            ("legacy-own", "session", "session-a"),
            ("project", "workspace", "workspace-a"),
            ("global", "global", ""),
            ("foreign-session", "conversation", "session-b"),
            ("foreign-project", "workspace", "workspace-b"),
            ("not-a-session", "session", "workspace-a"),
        ] {
            storage
                .add_memory(&MemoryEntry {
                    id: id.into(),
                    scope: scope.into(),
                    scope_id: scope_id.into(),
                    content: format!("fact-{id}"),
                    source: "user".into(),
                    created_at: "now".into(),
                    last_used: "now".into(),
                })
                .unwrap();
        }
        let context = storage.memory_context("session-a", "workspace-a").unwrap();
        assert_eq!(context.entries, 4);
        assert!(
            context.text.contains("fact-own")
                && context.text.contains("fact-legacy-own")
                && context.text.contains("fact-project")
                && context.text.contains("fact-global")
        );
        assert!(
            !context.text.contains("fact-foreign") && !context.text.contains("fact-not-a-session")
        );
        assert!(context.text.contains("source: user"));
        storage
            .add_memory(&MemoryEntry {
                id: "large".into(),
                scope: "conversation".into(),
                scope_id: "session-a".into(),
                content: "界".repeat(5000),
                source: "user".into(),
                created_at: "now".into(),
                last_used: "now".into(),
            })
            .unwrap();
        let context = storage.memory_context("session-a", "workspace-a").unwrap();
        assert!(context.text.chars().count() <= 2000);
        assert!(context.text.contains("[trimmed]"));
        assert_eq!(
            context.text,
            storage
                .memory_context("session-a", "workspace-a")
                .unwrap()
                .text,
            "context selection is deterministic"
        );
    }

    #[test]
    fn edit_truncates_later_messages() {
        let s = Storage::open_in_memory().unwrap();
        seed_thread(&s);
        assert!(s.update_message_content("ce", "e1", "edited").unwrap());
        assert_eq!(
            s.get_message("ce", "e1").unwrap().unwrap().content,
            "edited"
        );
        assert_eq!(s.delete_messages_after("ce", "e1").unwrap(), 2);
        let rest = s.messages_for("ce").unwrap();
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[1].content, "edited");
    }

    #[test]
    fn attachments_and_tool_audit_roundtrip() {
        let s = Storage::open_in_memory().unwrap();
        seed_thread(&s);
        s.add_attachment(&Attachment {
            id: "a1".into(),
            conversation_id: "ce".into(),
            filename: "notes.txt".into(),
            mime: "text/plain".into(),
            size_bytes: 3,
            text_excerpt: "abc".into(),
            kind: "text".into(),
            status: "ready".into(),
            created_at: "2026-01-01T00:00:05Z".into(),
        })
        .unwrap();
        assert_eq!(s.attachments_for("ce").unwrap().len(), 1);
        s.record_tool_execution(&ToolExecution {
            id: "t1".into(),
            conversation_id: "ce".into(),
            tool: "read_file".into(),
            args: "{}".into(),
            result: "ok".into(),
            approved: true,
            created_at: "2026-01-01T00:00:06Z".into(),
        })
        .unwrap();
        let execs = s.tool_executions_for("ce", 10).unwrap();
        assert_eq!(execs.len(), 1);
        assert!(execs[0].approved);
    }

    #[test]
    fn artifact_roundtrip() {
        let s = Storage::open_in_memory().unwrap();
        seed_thread(&s);
        s.record_artifact(&ArtifactRow {
            id: "ar1".into(),
            conversation_id: "ce".into(),
            filename: "f.csv".into(),
            path: "/tmp/f.csv".into(),
            mime: "text/csv".into(),
            size_bytes: 12,
            created_at: "2026-01-01T00:00:07Z".into(),
        })
        .unwrap();
        let all = s.artifacts_for("ce").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(s.get_artifact("ar1").unwrap().unwrap().mime, "text/csv");
        assert!(s.get_artifact("nope").unwrap().is_none());
    }

    #[test]
    fn output_timing_migrates_legacy_metrics_and_roundtrips_new_basis() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE generation_metrics(
            message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
            model_id TEXT NOT NULL, prompt_tokens INTEGER NOT NULL,
            generated_tokens INTEGER NOT NULL, gen_ms INTEGER NOT NULL,
            ttft_ms INTEGER NOT NULL, gen_tps REAL NOT NULL, created_at TEXT NOT NULL);
            INSERT INTO generation_metrics VALUES('old','conv','model',100,20,10000,2000,2.0,'then');").unwrap();
        let storage = Storage { conn };
        storage.migrate().unwrap();
        let legacy = storage.metrics_for("conv").unwrap().remove(0);
        assert_eq!(legacy.gen_tps, 2.0);
        assert!(
            legacy.timing.is_none(),
            "legacy overall rate must not gain a visible-output label"
        );
        let timing = crate::inference::OutputTiming {
            output_tps: Some(20.0),
            output_tokens: 20,
            output_ms: 1000,
            first_visible_ms: Some(5000),
            thinking_ms: Some(2000),
            total_ms: 6300,
            token_basis: "tokenizer".into(),
            ..crate::inference::OutputTiming::default()
        };
        storage
            .record_metric(&GenerationMetric {
                message_id: "new".into(),
                conversation_id: "conv".into(),
                model_id: "model".into(),
                prompt_tokens: 100,
                generated_tokens: 200,
                gen_ms: 6300,
                ttft_ms: 5000,
                gen_tps: 20.0,
                timing: Some(timing),
                created_at: "now".into(),
            })
            .unwrap();
        let metrics = storage.metrics_for("conv").unwrap();
        assert_eq!(metrics.len(), 2);
        assert!(metrics[0].timing.is_none());
        let output = metrics[1].timing.as_ref().unwrap();
        assert_eq!(output.basis, "visible_output_v1");
        assert_eq!(output.output_tps, Some(20.0));
        assert_eq!(output.output_tokens, 20);
        assert_eq!(output.thinking_ms, Some(2000));
    }
}
