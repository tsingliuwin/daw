//! Shared driver for OA write tools — the human-confirmation gate that parks a
//! write operation until the user approves/cancels it from the UI.
//!
//! (Migrated from lakemind's `ddl_shared.rs`. The oneshot-channel confirmation
//! mechanism is verbatim — only the actual "execute" step changed: lakemind ran
//! a DuckDB DDL batch, the OA app calls a tool-supplied async closure that
//! talks to the OA backend.)
//!
//! ## How it works
//!
//! 1. The tool calls [`OaWriteShared::run`], passing a description of the
//!    pending action (`summary_pending` + `detail`) and an async `execute`
//!    closure.
//! 2. If `confirm_mode == "变更前确认"`, the tool is parked: a oneshot channel
//!    is registered in `AppState::pending_confirmations` under
//!    `{task_id}:{tool_call_id}`, and `emit_tool_awaiting` tells the frontend
//!    to render an inline confirm UI.
//! 3. `rx.await` blocks the current `call()`. The user's confirm/cancel (from
//!    the frontend `resolve_tool_confirmation` command) sends a
//!    `ConfirmDecision` that unblocks the closure.
//! 4. On approval, `execute` runs and its result is emitted. On cancel, an
//!    error result is emitted and the tool returns `Err`.
//!
//! For `confirm_mode != "变更前确认"` (e.g. "自动执行"), `execute` runs
//! immediately with no gate.

use tokio::sync::oneshot;

use super::super::error::ToolError;
use super::super::events::{emit_tool_awaiting, emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

/// Shared state for OA write tools.
#[derive(Clone)]
pub(crate) struct OaWriteShared {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
    pub(crate) confirm_mode: String,
}

impl OaWriteShared {
    /// Drive one OA write operation end-to-end respecting the confirm mode.
    ///
    /// - `tool_name`/`tool_prefix` identify the tool in the transcript.
    /// - `args` is forwarded to the UI via the tool_call segment.
    /// - `summary_pending` is the short label shown while awaiting.
    /// - `detail` is the human-readable description of the pending action,
    ///   shown in the awaiting-confirm UI (e.g. "将提交 张三 的请假单
    ///   2026-08-03 → 2026-08-05 共 3 天").
    /// - `execute` runs the actual write against the OA backend on approval;
    ///   it returns `(summary_ok, payload)` where `payload` is the structured
    ///   result shown in the tool-result card.
    pub(crate) async fn run<F, Fut>(
        &self,
        tool_name: &str,
        tool_prefix: &str,
        args: serde_json::Value,
        summary_pending: String,
        detail: String,
        execute: F,
    ) -> Result<String, ToolError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(String, serde_json::Value), String>> + Send + 'static,
    {
        let call_id = next_tool_id(tool_prefix);
        emit_tool_call(&self.window, &self.task_id, &call_id, tool_name, args);

        // "变更前确认": park until the user decides. Any other value falls
        // through to immediate execution.
        if self.confirm_mode == "变更前确认" {
            let (tx, rx) = oneshot::channel::<crate::state::ConfirmDecision>();
            {
                let key = format!("{}:{}", self.task_id, call_id);
                let mut pending = self.app_state.pending_confirmations.lock().await;
                pending.insert(key.clone(), crate::state::PendingConfirmation { tx });
            }
            // Notify the UI this step is awaiting the user.
            emit_tool_awaiting(
                &self.window,
                &self.task_id,
                &call_id,
                summary_pending.clone(),
                detail.clone(),
            );

            let decision = rx.await;
            match decision {
                Ok(d) if d.approved => {
                    // fall through to execute below
                }
                _ => {
                    let msg = "用户已取消此操作".to_string();
                    emit_tool_result(
                        &self.window,
                        &self.task_id,
                        &call_id,
                        "error",
                        msg.clone(),
                        None,
                        None,
                        None,
                        None,
                    );
                    return Err(ToolError(msg));
                }
            }
        }

        let start = std::time::Instant::now();
        match execute().await {
            Ok((summary, payload)) => {
                let elapsed = start.elapsed().as_millis() as u64;
                emit_tool_result(
                    &self.window,
                    &self.task_id,
                    &call_id,
                    "ok",
                    summary.clone(),
                    None,
                    Some(payload),
                    Some(elapsed),
                    None,
                );
                Ok(summary)
            }
            Err(msg) => {
                let elapsed = start.elapsed().as_millis() as u64;
                emit_tool_result(
                    &self.window,
                    &self.task_id,
                    &call_id,
                    "error",
                    msg.clone(),
                    None,
                    None,
                    Some(elapsed),
                    None,
                );
                Err(ToolError(msg))
            }
        }
    }
}
