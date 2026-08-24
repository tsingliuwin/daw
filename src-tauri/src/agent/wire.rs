//! Wire protocol: ordered segment transcript streamed to the frontend.
//!
//! (Ported from the earlier data-lake prototype, SQL/data-viz specifics removed:
//!   - `Segment::Tool` dropped its `sql: Option<String>` and
//!     `table: Option<SqlResult>` fields — OA tools don't run SQL. They are
//!     replaced by `detail: Option<String>` (a human-readable summary of the
//!     pending/performed action, shown in the awaiting-confirm UI) and
//!     `payload: Option<serde_json::Value>` (the structured OA result — a leave
//!     balance record, a submitted request, an approval preview, ...).
//!   - `Segment::Chart` was removed entirely — the OA app has no inline chart
//!     rendering.
//! The reasoning/text/tool/error lifecycle is unchanged.)
//!
//! An assistant message is a list of `Segment`s in arrival order:
//!   reasoning → tool → reasoning → tool → text (final answer)
//! Each tool is one `Segment::Tool` whose status transitions running → ok|error
//! when the matching tool_result event arrives (updated in place by id).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::model::SqlResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Segment {
    /// Model thinking. Accumulated from reasoning deltas.
    Reasoning { id: String, text: String },
    /// One tool call + its result merged into a single logical step.
    /// `status` goes running → ok|error|awaiting when the tool_result event
    /// arrives.
    #[serde(rename_all = "camelCase")]
    Tool {
        id: String,
        tool: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        status: String, // "running" | "ok" | "error" | "awaiting"
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// Human-readable description of the (pending or performed) action.
        /// For write tools in "变更前确认" mode this is what the awaiting-confirm
        /// UI shows the user before they approve/cancel.
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// Structured result payload. Free-form JSON so each tool shapes its
        /// own payload; the frontend renders it tool-specifically.
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        /// Compact structured audit fields (e.g. the *executed* SQL for the
        /// SQL tools). Persisted with the transcript so 历史排查 / agent 调优 can
        /// consume it without string-parsing summaries.
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<serde_json::Value>,
    },
    /// Visible answer text (Markdown). Accumulated from text deltas.
    Text { id: String, text: String },
    /// Inline chart (data-analysis scenario). Emitted by render_chart tool.
    /// The frontend renders it as an interactive ECharts card; the model can
    /// embed `{{chart:<id>}}` markers in its text to reference it inline.
    #[serde(rename_all = "camelCase")]
    Chart {
        id: String,
        chart_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        x_field: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        y_fields: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        right_y_fields: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y_field_labels: Option<HashMap<String, String>>,
        table: SqlResult,
    },
    /// Terminal/agent execution error.
    Error { id: String, text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStreamEvent {
    pub task_id: String,
    // "reasoning" | "text" | "tool_call" | "tool_result" | "retry" | "done" | "error" | "usage"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment: Option<Segment>,
    // ---- kind = "retry"（速率限制自动重试提示，瞬时事件、不落消息段）----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_secs: Option<u64>,
}

/// Minimal view of a stored message for rebuilding the LLM history.
/// Both `content` (legacy) and `segments` (new) are optional + default so the
/// DTO tolerates either persisted shape.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChatMessageDto {
    pub role: String, // "user" | "assistant"
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub segments: Option<Vec<Segment>>,
    pub ts: i64,
}
