//! Event-emission layer: pushes structured `AgentStreamEvent`s to the frontend
//! via `window.emit("agent-event", ...)`. All tools and the streaming runner go
//! through these helpers so the wire format stays consistent.
//!
//! (Ported from the earlier data-lake prototype, SQL/chart specifics removed:
//!   - `emit_tool_result` / `emit_tool_awaiting` take `detail` + `payload`
//!     instead of `sql` + `table: SqlResult`.
//!   - `emit_chart` is gone (no inline charts in this app).
//! The usage helpers, the tool-call id generator, and the abort/done bookkeeping
//! are unchanged.)

use tauri::Emitter;
use std::collections::HashMap;

use super::wire::{AgentStreamEvent, Segment};
use crate::model::SqlResult;
use crate::usage::{self};

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Monotonic counter that guarantees unique tool-call ids.
static TOOL_CALL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn next_tool_id(prefix: &str) -> String {
    let n = TOOL_CALL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    format!("tool-{prefix}-{}-{n}", now_ms())
}

pub(crate) fn emit_event(
    window: &tauri::Window,
    task_id: &str,
    kind: &str,
    text: Option<String>,
    segment: Option<Segment>,
) {
    let _ = window.emit(
        "agent-event",
        AgentStreamEvent {
            task_id: task_id.to_string(),
            kind: kind.to_string(),
            text,
            segment,
            attempt: None,
            max_attempts: None,
            delay_secs: None,
        },
    );
}

/// Emit a partial reasoning/text delta to be appended to the current segment of
/// that type on the frontend.
pub(crate) fn emit_delta(window: &tauri::Window, task_id: &str, kind: &str, text: &str) {
    emit_event(window, task_id, kind, Some(text.to_string()), None);
}

/// Emit a rate-limit retry notice (kind = "retry"). Ephemeral by design: the
/// frontend renders it as a transient status banner and it never becomes a
/// transcript text segment——重试提示不能混进最终答复与 LLM 历史回放。
pub(crate) fn emit_retry_notice(
    window: &tauri::Window,
    task_id: &str,
    attempt: u32,
    max_attempts: u32,
    delay_secs: u64,
) {
    let _ = window.emit(
        "agent-event",
        AgentStreamEvent {
            task_id: task_id.to_string(),
            kind: "retry".to_string(),
            text: None,
            segment: None,
            attempt: Some(attempt),
            max_attempts: Some(max_attempts),
            delay_secs: Some(delay_secs),
        },
    );
}

/// Emit a `tool_call` segment (status: running) — opens a new tool step in the
/// transcript.
pub(crate) fn emit_tool_call(
    window: &tauri::Window,
    task_id: &str,
    id: &str,
    tool: &str,
    args: serde_json::Value,
) {
    emit_event(
        window,
        task_id,
        "tool_call",
        None,
        Some(Segment::Tool {
            id: id.to_string(),
            tool: tool.to_string(),
            args: Some(args),
            status: "running".to_string(),
            summary: None,
            detail: None,
            payload: None,
            elapsed_ms: None,
            result: None,
        }),
    );
}

/// Emit a `tool_result` — merged into the matching tool segment by id, flipping
/// its status to ok|error and attaching the result payload.
pub(crate) fn emit_tool_result(
    window: &tauri::Window,
    task_id: &str,
    id: &str,
    status: &str, // "ok" | "error"
    summary: String,
    detail: Option<String>,
    payload: Option<serde_json::Value>,
    elapsed_ms: Option<u64>,
    result: Option<String>,
) {
    emit_event(
        window,
        task_id,
        "tool_result",
        None,
        Some(Segment::Tool {
            id: id.to_string(),
            tool: String::new(), // frontend merges by id; tool name already set
            args: None,
            status: status.to_string(),
            summary: Some(summary),
            detail,
            payload,
            elapsed_ms,
            result,
        }),
    );
}

/// Emit a `tool_result` carrying `status: "awaiting"` — marks the tool segment
/// as parked pending the user's confirm/cancel decision in "变更前确认" mode.
/// `detail` is the human-readable description of what the tool is about to do,
/// shown in the inline confirm UI.
pub(crate) fn emit_tool_awaiting(
    window: &tauri::Window,
    task_id: &str,
    id: &str,
    summary: String,
    detail: String,
) {
    emit_event(
        window,
        task_id,
        "tool_result",
        None,
        Some(Segment::Tool {
            id: id.to_string(),
            tool: String::new(),
            args: None,
            status: "awaiting".to_string(),
            summary: Some(summary),
            detail: Some(detail),
            payload: None,
            elapsed_ms: None,
            result: None,
        }),
    );
}

/// Emit an inline chart segment (data-analysis scenario). The frontend renders
/// it as an interactive ECharts card; the model references it via `{{chart:<id>}}`.
pub(crate) fn emit_chart(
    window: &tauri::Window,
    task_id: &str,
    id: &str,
    chart_type: &str,
    title: Option<&str>,
    x_field: Option<&str>,
    y_fields: Option<&[String]>,
    right_y_fields: Option<&[String]>,
    y_field_labels: Option<&HashMap<String, String>>,
    table: SqlResult,
) {
    emit_event(
        window,
        task_id,
        "chart",
        None,
        Some(Segment::Chart {
            id: id.to_string(),
            chart_type: chart_type.to_string(),
            title: title.map(|s| s.to_string()),
            x_field: x_field.map(|s| s.to_string()),
            y_fields: y_fields.map(|v| v.to_vec()),
            right_y_fields: right_y_fields.map(|v| v.to_vec()),
            y_field_labels: y_field_labels.cloned(),
            table,
        }),
    );
}

/// Emit a usage *estimate* event — sent before/during streaming, before the
/// API's exact FinalResponse usage arrives.
pub(crate) fn emit_usage_estimate(
    window: &tauri::Window,
    task_id: &str,
    prompt_tokens_est: u64,
    output_tokens_est: u64,
    preamble_raw: u64,
    tools_raw: u64,
) {
    let _ = window.emit(
        "agent-event",
        AgentStreamEvent {
            task_id: task_id.to_string(),
            kind: "usage".to_string(),
            text: Some(
                serde_json::to_string(&serde_json::json!({
                    "isEstimate": true,
                    "promptTokens": prompt_tokens_est,
                    "completionTokens": output_tokens_est,
                    "estPreambleRaw": preamble_raw,
                    "estToolsRaw": tools_raw,
                }))
                .unwrap_or_default(),
            ),
            segment: None,
            attempt: None,
            max_attempts: None,
            delay_secs: None,
        },
    );
}

/// Emit a *real* usage event from a FinalResponse (one per LLM call within the
/// multi-turn run).
pub(crate) fn emit_usage_real(
    window: &tauri::Window,
    task_id: &str,
    n: usage::NormalizedUsage,
    k_sample: Option<f64>,
    run_completion_tokens: u64,
    preamble_raw: u64,
    tools_raw: u64,
) {
    let mut payload = serde_json::json!({
        "isEstimate": false,
        "promptTokens": n.prompt_tokens,
        "completionTokens": n.completion_tokens,
        "runCompletionTokens": run_completion_tokens,
        "cacheReadTokens": n.cache_read_tokens,
        "cacheCreationTokens": n.cache_creation_tokens,
        "freshInputTokens": n.fresh_input_tokens,
        "estPreambleRaw": preamble_raw,
        "estToolsRaw": tools_raw,
    });
    if let Some(k) = k_sample {
        payload["kSample"] = serde_json::json!(k);
    }
    let _ = window.emit(
        "agent-event",
        AgentStreamEvent {
            task_id: task_id.to_string(),
            kind: "usage".to_string(),
            text: Some(serde_json::to_string(&payload).unwrap_or_default()),
            segment: None,
            attempt: None,
            max_attempts: None,
            delay_secs: None,
        },
    );
}

/// Emit a run *summary* at the end of one agent run.
pub(crate) fn emit_usage_run_summary(
    window: &tauri::Window,
    task_id: &str,
    run_output_tokens: u64,
    run_elapsed_ms: u64,
) {
    let tok_per_sec = if run_elapsed_ms > 0 {
        let secs = (run_elapsed_ms as f64) / 1000.0;
        (run_output_tokens as f64 / secs.max(0.001)).round() as u64
    } else {
        0
    };
    let _ = window.emit(
        "agent-event",
        AgentStreamEvent {
            task_id: task_id.to_string(),
            kind: "usage".to_string(),
            text: Some(
                serde_json::to_string(&serde_json::json!({
                    "isEstimate": false,
                    "turnComplete": true,
                    "runOutputTokens": run_output_tokens,
                    "runElapsedMs": run_elapsed_ms,
                    "tokPerSec": tok_per_sec,
                }))
                .unwrap_or_default(),
            ),
            segment: None,
            attempt: None,
            max_attempts: None,
            delay_secs: None,
        },
    );
}
