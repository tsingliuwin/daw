//! Event-emission layer: pushes structured `AgentStreamEvent`s to the frontend
//! via `window.emit("agent-event", ...)`. All tools and the streaming runner go
//! through these helpers so the wire format stays consistent.
//!
//! (Migrated from lakemind with the SQL/chart specifics removed:
//!   - `emit_tool_result` / `emit_tool_awaiting` take `detail` + `payload`
//!     instead of `sql` + `table: SqlResult`.
//!   - `emit_chart` is gone (no inline chart in the OA app).
//! The usage helpers, the tool-call id generator, and the abort/done bookkeeping
//! are unchanged.)

use tauri::Emitter;

use super::wire::{AgentStreamEvent, Segment};
use crate::usage::{self};

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Monotonic counter that guarantees unique tool-call ids.
static TOOL_CALL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn next_tool_id(prefix: &str) -> String {
    let n = TOOL_CALL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    format!("tool-{prefix}-{}-{n}", now_ms())
}

pub(super) fn emit_event(
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
        },
    );
}

/// Emit a partial reasoning/text delta to be appended to the current segment of
/// that type on the frontend.
pub(super) fn emit_delta(window: &tauri::Window, task_id: &str, kind: &str, text: &str) {
    emit_event(window, task_id, kind, Some(text.to_string()), None);
}

/// Emit a `tool_call` segment (status: running) — opens a new tool step in the
/// transcript.
pub(super) fn emit_tool_call(
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
pub(super) fn emit_tool_result(
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
pub(super) fn emit_tool_awaiting(
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

/// Emit a usage *estimate* event — sent before/during streaming, before the
/// API's exact FinalResponse usage arrives.
pub(super) fn emit_usage_estimate(
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
        },
    );
}

/// Emit a *real* usage event from a FinalResponse (one per LLM call within the
/// multi-turn run).
pub(super) fn emit_usage_real(
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
        },
    );
}

/// Emit a run *summary* at the end of one agent run.
pub(super) fn emit_usage_run_summary(
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
        },
    );
}
