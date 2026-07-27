//! Data transfer objects shared between Rust and the SolidJS frontend.
//!
//! These structs are the wire format: `src/lib/types.ts` mirrors them 1:1.
//! Keep both in sync when changing.
//!
//! (Migrated from lakemind with all DuckDB/data-lake types removed — the OA
//! tools carry structured results as `serde_json::Value` payloads instead of
//! `SqlResult`, so no row/column DTO is needed here.)

use serde::{Deserialize, Serialize};

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

/// Normalized log category. The OA app keeps the same fixed taxonomy as
/// lakemind minus the data-lake-only buckets; `agent` / `system` / `ui` /
/// `link` stay relevant. New buckets can be appended freely — old rows still
/// parse, and the frontend filters on whatever category strings it sees.
#[allow(dead_code)]
pub const LOG_CATEGORIES: &[&str] = &["agent", "system", "ui", "link", "oa"];

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
