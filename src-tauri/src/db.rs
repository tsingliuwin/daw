//! Global metadata store: `~/.aioa/aioa.db` (SQLite via `rusqlite`).
//!
//! Holds the registries that are *not* business data themselves:
//!   * `workspaces` — registered workspace directories (an isolated project:
//!     its own task list, its own linked OA systems).
//!   * `tasks`      — per-workspace chat task index (content lives in files).
//!   * `config`     — key/value user settings.
//!   * `logs`       — the unified, queryable log store.
//!
//! (Migrated from lakemind's `db.rs` with all data-lake tables removed:
//! `sources`, `object_defs`, `db_connection_tables` are gone. The
//! `db_connections` + `workspace_connections` tables are kept as a generic
//! "linked external system" registry — future OA integrations store their
//! connection config there.)

use std::fs;
use std::path::PathBuf;
use rusqlite::Connection;

use crate::model::DataSourceConfig;

/// Get the system home directory
pub fn get_home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .ok()
}

/// Get the global config path ~/.aioa/
pub fn get_aioa_dir() -> Result<PathBuf, String> {
    let mut path = get_home_dir().ok_or("Could not resolve home directory".to_string())?;
    path.push(".aioa");
    Ok(path)
}

/// Get the global sqlite database file path ~/.aioa/aioa.db
pub fn get_db_path() -> Result<PathBuf, String> {
    let mut path = get_aioa_dir()?;
    path.push("aioa.db");
    Ok(path)
}

/// Get the per-space, per-user chat content directory
/// `~/.aioa/<space_id>/<user_id>/chats/`.
///
/// The "personal" space (`space_id = "personal"`) always uses
/// `user_id = "default"`; each joined enterprise uses its UUID as `space_id`
/// and the enterprise user's `username` as `user_id`. The directory is created
/// (idempotent) so any caller - read or write - can rely on it existing.
pub fn get_chats_dir(space_id: &str, user_id: &str) -> Result<PathBuf, String> {
    let mut path = get_aioa_dir()?;
    path.push(space_id);
    path.push(user_id);
    path.push("chats");
    fs::create_dir_all(&path).map_err(|e| {
        format!("Failed to create chats directory for space {space_id}/user {user_id}: {e}")
    })?;
    Ok(path)
}

/// Establish connection to sqlite database.
///
/// Each call opens a fresh connection (the app fans out concurrent reads/writes
/// from many commands). Two pragmas make that safe under concurrency:
/// - `busy_timeout = 5000`: wait up to 5s for a lock instead of failing
///   instantly with SQLITE_BUSY.
/// - `journal_mode = WAL`: readers never block writers (and vice-versa).
pub fn get_db_conn() -> Result<Connection, String> {
    let db_path = get_db_path()?;
    let conn = Connection::open(&db_path)
        .map_err(|e| format!("Failed to open SQLite database: {e}"))?;
    let _ = conn.pragma_update(None, "busy_timeout", 5000);
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    Ok(conn)
}

// ---------------------------------------------------------------------------
// config: key/value user settings
// ---------------------------------------------------------------------------

/// Read a config value. Returns `None` for missing or empty values.
pub fn get_config(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM config WHERE key = ?", [key], |r| r.get(0))
        .ok();
    Ok(v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// Set (upsert) a config value.
pub fn set_config(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO config (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// schema initialization
// ---------------------------------------------------------------------------

/// Initialize central directory structure and table schemas. Idempotent.
pub fn init_global_db() -> Result<(), String> {
    // Content directory for chat task files, scoped per space *and* per user.
    // The default `personal` space (user "default") is always created on
    // startup; enterprise spaces are created on demand by
    // `get_chats_dir(<uuid>, <username>)` when their tasks are first read/written.
    get_chats_dir("personal", "default")?;
    migrate_legacy_chats();

    let conn = get_db_conn()?;
    let _ = conn.execute("PRAGMA foreign_keys = ON;", []);

    // workspaces registry
    conn.execute(
        "CREATE TABLE IF NOT EXISTS workspaces (
            path TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| format!("Failed to create workspaces table: {e}"))?;

    // tasks index (content lives in chats/<id>.json). `kind` is currently
    // always "chat" but kept as a column so future task flavors (e.g. a
    // standalone approval form) slot in without a migration. `space_id` +
    // `user_id` together isolate tasks per space and per user within it.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            workspace_path TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            saved INTEGER NOT NULL,
            model_id TEXT,
            token_usage TEXT,
            space_id TEXT NOT NULL DEFAULT 'personal',
            user_id TEXT NOT NULL DEFAULT 'default',
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create tasks table: {e}"))?;

    // Idempotent migrations: add the `space_id` and `user_id` columns to tasks
    // for pre-multispace / pre-multiuser databases. Fresh DBs already have them
    // via CREATE TABLE above; ALTER fails when the column already exists, which
    // we silently ignore.
    let _ = conn.execute(
        "ALTER TABLE tasks ADD COLUMN space_id TEXT NOT NULL DEFAULT 'personal'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE tasks ADD COLUMN user_id TEXT NOT NULL DEFAULT 'default'",
        [],
    );

    // config: key/value user settings
    conn.execute(
        "CREATE TABLE IF NOT EXISTS config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| format!("Failed to create config table: {e}"))?;

    // logs: the unified, queryable log store.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS logs (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            ts       INTEGER NOT NULL,
            level    TEXT NOT NULL,
            category TEXT NOT NULL,
            message  TEXT NOT NULL,
            detail   TEXT,
            workspace TEXT,
            task_id  TEXT
        )",
        [],
    )
    .map_err(|e| format!("Failed to create logs table: {e}"))?;
    let _ = conn.execute("CREATE INDEX IF NOT EXISTS idx_logs_ts ON logs(ts DESC);", []);
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_logs_cat_ts ON logs(category, ts DESC);",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_logs_level_ts ON logs(level, ts DESC);",
        [],
    );

    // oa_connections: linked external OA systems (future: DingTalk / Feishu /
    // WeCom / self-built OA). Generic key/value-ish so any system's config
    // (base_url, tenant_id, app_key, app_secret, ...) can live here. M1 leaves
    // it empty — the LocalOaBackend covers the demo — but the table exists so
    // the plumbing is ready.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS oa_connections (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            system_type   TEXT NOT NULL,
            base_url      TEXT NOT NULL DEFAULT '',
            created_at    INTEGER NOT NULL,
            options       TEXT
        )",
        [],
    )
    .map_err(|e| format!("Failed to create oa_connections table: {e}"))?;

    // workspace_connections: many-to-many between workspaces and oa_connections.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS workspace_connections (
            workspace_path TEXT NOT NULL,
            connection_id  TEXT NOT NULL,
            PRIMARY KEY (workspace_path, connection_id),
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE,
            FOREIGN KEY(connection_id) REFERENCES oa_connections(id) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create workspace_connections table: {e}"))?;

    // db_connections: 数据分析场景的外部数据库连接（postgres/mysql/sqlite）。
    // 与 oa_connections 分开——这里是 DuckDB ATTACH 用的数据源连接。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS db_connections (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            db_type       TEXT NOT NULL,
            host          TEXT NOT NULL,
            port          INTEGER NOT NULL,
            database_name TEXT NOT NULL,
            username      TEXT NOT NULL,
            password      TEXT NOT NULL,
            ssl_mode      TEXT NOT NULL DEFAULT 'disable',
            created_at    INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| format!("Failed to create db_connections table: {e}"))?;

    // workspace_db_connections: 工作区↔数据源多对多关联。
    // link 时 ATTACH 到 DuckDB 会话，unlink 时 DETACH。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS workspace_db_connections (
            workspace_path TEXT NOT NULL,
            connection_id  TEXT NOT NULL,
            PRIMARY KEY (workspace_path, connection_id),
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE,
            FOREIGN KEY(connection_id) REFERENCES db_connections(id) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create workspace_db_connections table: {e}"))?;

    // Seed the default workspace on first run.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
        .unwrap_or(0);
    if count == 0 {
        let now = now_ms();
        conn.execute(
            "INSERT INTO workspaces (path, name, created_at) VALUES ('DefaultProject', 'DefaultProject', ?)",
            [now],
        )
        .map_err(|e| format!("Failed to insert default workspace: {e}"))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// logs: the unified, queryable log store
// ---------------------------------------------------------------------------

/// Insert one log row. Returns the new autoincrement id.
///
/// Called from the tracing `SqliteEmitLayer` (every backend event) and the
/// `append_log` Tauri command (every frontend log). Failures here MUST be
/// non-fatal — logging can never take the app down — so callers swallow the
/// error.
pub fn insert_log(conn: &Connection, rec: &crate::model::LogRecord) -> Result<i64, String> {
    let detail = rec
        .detail
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()));
    conn.execute(
        "INSERT INTO logs (ts, level, category, message, detail, workspace, task_id)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            rec.ts,
            rec.level.as_str(),
            rec.category,
            rec.message,
            detail,
            rec.workspace,
            rec.task_id,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// Query logs with optional filters, newest-first. `limit` defaults to 200 when
/// non-positive.
pub fn query_logs(
    conn: &Connection,
    filter: &crate::model::LogFilter,
) -> Result<Vec<crate::model::LogRecord>, String> {
    let limit = if filter.limit <= 0 { 200 } else { filter.limit };
    let offset = filter.offset.max(0);

    let mut where_parts: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(cats) = &filter.categories {
        if !cats.is_empty() {
            let placeholders = vec!["?"; cats.len()].join(",");
            where_parts.push(format!("category IN ({placeholders})"));
            for c in cats {
                params.push(Box::new(c.clone()));
            }
        }
    }
    if let Some(levels) = &filter.levels {
        if !levels.is_empty() {
            let placeholders = vec!["?"; levels.len()].join(",");
            where_parts.push(format!("level IN ({placeholders})"));
            for l in levels {
                params.push(Box::new(l.clone()));
            }
        }
    }
    if let Some(from) = filter.from_ts {
        where_parts.push("ts >= ?".to_string());
        params.push(Box::new(from));
    }
    if let Some(to) = filter.to_ts {
        where_parts.push("ts <= ?".to_string());
        params.push(Box::new(to));
    }
    if let Some(kw) = filter.keyword.as_ref().filter(|s| !s.trim().is_empty()) {
        where_parts.push("LOWER(message) LIKE LOWER(?)".to_string());
        params.push(Box::new(format!("%{}%", kw.trim())));
    }

    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    let sql = format!(
        "SELECT id, ts, level, category, message, detail, workspace, task_id
         FROM logs {where_clause}
         ORDER BY ts DESC, id DESC
         LIMIT ? OFFSET ?"
    );
    params.push(Box::new(limit));
    params.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            let level_str: String = row.get(2)?;
            let detail_str: Option<String> = row.get(5).ok();
            let detail = detail_str
                .filter(|s| !s.is_empty())
                .and_then(|s| serde_json::from_str(&s).ok());
            Ok(crate::model::LogRecord {
                id: Some(row.get(0)?),
                ts: row.get(1)?,
                level: crate::model::LogLevel::from_db_str(&level_str),
                category: row.get(3)?,
                message: row.get(4)?,
                detail,
                workspace: row.get(6).ok(),
                task_id: row.get(7).ok(),
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        if let Ok(rec) = r {
            out.push(rec);
        }
    }
    Ok(out)
}

/// Delete logs. `before = None` clears ALL logs; `Some(ts)` deletes rows with
/// `ts < before` (used for retention).
pub fn clear_logs(conn: &Connection, before: Option<i64>) -> Result<(), String> {
    match before {
        Some(ts) => {
            conn.execute("DELETE FROM logs WHERE ts < ?", [ts])
                .map_err(|e| e.to_string())?;
        }
        None => {
            conn.execute("DELETE FROM logs", []).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Current Unix-ms timestamp.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Best-effort one-time migration: legacy personal-mode stored chat files in
/// `~/.aioa/chats/` (pre-multispace) and later in `~/.aioa/personal/chats/`
/// (multispace, pre-userId). The per-user layout moves them under the personal
/// space for the default user at `~/.aioa/personal/default/chats/`. Move any
/// leftover `.json` files so existing users keep their history on upgrade.
/// Failures are swallowed - this must never block startup.
fn migrate_legacy_chats() {
    let Ok(aioa_dir) = get_aioa_dir() else {
        return;
    };
    let Ok(target) = get_chats_dir("personal", "default") else {
        return;
    };
    // Two legacy locations, both now consolidated under personal/default/chats.
    let legacy_dirs = [aioa_dir.join("chats"), aioa_dir.join("personal").join("chats")];
    for legacy in legacy_dirs {
        if !legacy.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&legacy) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(file_name) = p.file_name() else {
                continue;
            };
            let dest = target.join(file_name);
            if dest.exists() {
                continue;
            }
            // `rename` is atomic on the same volume (the paths share ~/.aioa,
            // so they always are here); fall back to copy+remove if it ever
            // fails (e.g. across volumes).
            if fs::rename(&p, &dest).is_err() {
                let _ = fs::copy(&p, &dest).and_then(|_| fs::remove_file(&p));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// db_connections: 数据分析数据源 CRUD
// ---------------------------------------------------------------------------

/// Row → DataSourceConfig 映射。
fn row_to_conn(r: &rusqlite::Row) -> rusqlite::Result<DataSourceConfig> {
    Ok(DataSourceConfig {
        id: r.get(0)?,
        name: r.get(1)?,
        db_type: r.get(2)?,
        host: r.get(3)?,
        port: r.get(4)?,
        database_name: r.get(5)?,
        username: r.get(6)?,
        password: r.get(7)?,
        ssl_mode: r.get(8)?,
        created_at: r.get(9)?,
    })
}

const CONN_COLS: &str = "id, name, db_type, host, port, database_name, username, password, ssl_mode, created_at";

pub fn list_db_connections() -> Result<Vec<DataSourceConfig>, String> {
    let conn = get_db_conn()?;
    let mut stmt = conn
        .prepare(&format!("SELECT {CONN_COLS} FROM db_connections ORDER BY created_at"))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_conn)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn get_db_connection(id: &str) -> Result<Option<DataSourceConfig>, String> {
    let conn = get_db_conn()?;
    let mut stmt = conn
        .prepare(&format!("SELECT {CONN_COLS} FROM db_connections WHERE id = ?"))
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([id]).map_err(|e| e.to_string())?;
    if let Some(r) = rows.next().map_err(|e| e.to_string())? {
        return Ok(Some(row_to_conn(r).map_err(|e| e.to_string())?));
    }
    Ok(None)
}

pub fn create_db_connection(r: &DataSourceConfig) -> Result<(), String> {
    let conn = get_db_conn()?;
    conn.execute(
        "INSERT INTO db_connections (id, name, db_type, host, port, database_name, username, password, ssl_mode, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![r.id, r.name, r.db_type, r.host, r.port, r.database_name, r.username, r.password, r.ssl_mode, r.created_at],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn update_db_connection(r: &DataSourceConfig) -> Result<(), String> {
    let conn = get_db_conn()?;
    conn.execute(
        "UPDATE db_connections SET name=?, db_type=?, host=?, port=?, database_name=?, username=?, password=?, ssl_mode=? WHERE id=?",
        rusqlite::params![r.name, r.db_type, r.host, r.port, r.database_name, r.username, r.password, r.ssl_mode, r.id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn delete_db_connection(id: &str) -> Result<(), String> {
    let conn = get_db_conn()?;
    conn.execute("DELETE FROM db_connections WHERE id = ?", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 列出某工作区已 link 的数据源。
pub fn list_workspace_db_connections(ws_path: &str) -> Result<Vec<DataSourceConfig>, String> {
    let conn = get_db_conn()?;
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name, c.db_type, c.host, c.port, c.database_name, c.username, c.password, c.ssl_mode, c.created_at
             FROM db_connections c
             INNER JOIN workspace_db_connections w ON c.id = w.connection_id
             WHERE w.workspace_path = ? ORDER BY c.created_at",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([ws_path], row_to_conn)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 按名称查工作区已 link 的单个数据源。
pub fn get_workspace_db_connection_by_name(ws_path: &str, name: &str) -> Result<Option<DataSourceConfig>, String> {
    let conns = list_workspace_db_connections(ws_path)?;
    Ok(conns.into_iter().find(|c| c.name == name))
}

pub fn link_workspace_db_connection(ws_path: &str, conn_id: &str) -> Result<(), String> {
    let conn = get_db_conn()?;
    conn.execute(
        "INSERT OR IGNORE INTO workspace_db_connections (workspace_path, connection_id) VALUES (?, ?)",
        rusqlite::params![ws_path, conn_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn unlink_workspace_db_connection(ws_path: &str, conn_id: &str) -> Result<(), String> {
    let conn = get_db_conn()?;
    conn.execute(
        "DELETE FROM workspace_db_connections WHERE workspace_path = ? AND connection_id = ?",
        rusqlite::params![ws_path, conn_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
