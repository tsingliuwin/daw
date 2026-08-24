//! Data transfer objects shared between Rust and the SolidJS frontend.
//!
//! These structs are the wire format: `src/lib/types.ts` mirrors them 1:1.
//! Keep both in sync when changing.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SQL query result (data-analysis scenario)
// ---------------------------------------------------------------------------

/// Result of an ad-hoc SQL execution. Returned to the frontend as the
/// `payload` of a `tool_result` event and rendered by `ResultTable.tsx`.
/// Mirrors `src/lib/types.ts` `SqlResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    /// Rows as heterogeneous JSON values (numbers, strings, null, ...).
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Number of rows actually returned (== rows.len()).
    pub row_count: usize,
    /// True when a SELECT exceeded the row cap and was truncated.
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// External data-source connection config. Stored in `settings.json` under the
/// `dataSources` array; DuckDB ATTACHes each one at startup.
/// Mirrors `src/lib/types.ts` `DataSourceConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceConfig {
    pub id: String,
    /// Display name; also the source of the DuckDB catalog alias (`db_<safe>`).
    pub name: String,
    #[serde(rename = "dbType")]
    pub db_type: String, // "postgres" | "mysql" | "sqlite"
    pub host: String,
    pub port: i32,
    /// For sqlite: the local file path (host/port/user/password unused).
    pub database_name: String,
    pub username: String,
    pub password: String,
    pub ssl_mode: String,
    /// Creation timestamp (Unix ms). Set by upsert_db_connection.
    #[serde(default)]
    pub created_at: i64,
    /// Database product: "postgresql" | "hologres" | "oceanbase" | "unknown".
    #[serde(default)]
    pub db_product: String,
    /// Library mode: "standard" | "external" | "unknown" (Hologres external db vs standard).
    #[serde(default)]
    pub db_mode: String,
}

/// Table registry record. Stores technical metadata for each registered remote table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRegistryEntry {
    pub id: String,
    pub workspace_path: String,
    pub connection_name: String,
    pub local_name: String,
    pub remote_schema: String,
    pub remote_table: String,
    pub db_type: String,
    pub db_product: String,
    pub db_mode: String,
    pub table_type: String,     // "native" | "foreign"
    pub access_mode: String,    // "catalog" | "pushdown"
    pub status: String,         // "available" | "unavailable_permanent" | "unavailable_temporary"
    pub unavailable_reason: Option<String>,
    pub last_explored: Option<i64>,
    pub kind: String,            // "table" | "view"
}

// ---------------------------------------------------------------------------
// Unified logging
// ---------------------------------------------------------------------------

/// Severity level for the unified log store. Mirrors `tracing` levels. Stored
/// as a lowercase TEXT column in SQLite and sent over the wire as a lowercase
/// string — keep `src/lib/types.ts` `UnifiedLog` in sync.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Map a `tracing::Level` to our coarse-grained store level.
    pub fn from_tracing(level: &tracing::Level) -> Self {
        match *level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            _ => Self::Debug,
        }
    }
    /// Lowercase string written into the SQLite `logs.level` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
    /// Parse from the string stored in the `logs.level` column; falls back to Info.
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "debug" => Self::Debug,
            "warn" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

/// Normalized log category. Same fixed taxonomy as the earlier data-lake
/// prototype minus the data-lake-only buckets; `agent` / `system` / `ui` /
/// `link` stay relevant. New buckets can be appended freely — old rows still
/// parse, and the frontend filters on whatever category strings it sees.
#[allow(dead_code)]
pub const LOG_CATEGORIES: &[&str] = &["agent", "system", "ui", "link", "oa", "sql"];

/// One row of the unified `logs` table. The wire format mirrored 1:1 by
/// `src/lib/types.ts` `UnifiedLog`. `detail` is a free-form JSON object for
/// structured fields that vary per category — the message field stays a
/// single-line human summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    /// Row id (autoincrement in SQLite). `None` on insert, filled after write /
    /// on read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// Unix-ms timestamp.
    pub ts: i64,
    pub level: LogLevel,
    /// One of [`LOG_CATEGORIES`] (or any free-form string).
    pub category: String,
    /// Single-line human-readable summary.
    pub message: String,
    /// Opaque JSON object of structured detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    /// Associated workspace path (`None` for global / startup logs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Associated task id (agent logs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// Filter clause for `db::query_logs`. Every field is optional; `None` means
/// "no constraint on this dimension".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFilter {
    /// Restrict to these categories (OR). `None` / empty = all categories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    /// Restrict to these levels (OR). `None` / empty = all levels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<Vec<String>>,
    /// Inclusive lower bound (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_ts: Option<i64>,
    /// Inclusive upper bound (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_ts: Option<i64>,
    /// Substring match against `message` (case-insensitive LIKE).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    /// Page size. Defaults to 200 when unset by the backend.
    pub limit: i64,
    /// Page offset.
    #[serde(default)]
    pub offset: i64,
}
