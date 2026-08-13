//! SkillContext — Skill 的 Tool 在 `call()` 里访问基座能力的入口。
//!
//! 注入：AppState + window + task_id + confirm 通道。Skill 的 Tool 构造时
//! 持有 Context（或其 clone），在 `call()` 内通过它发射事件、挂起确认等。
//! 当前为占位结构——基座的能力（confirm/abort/event）已在 AppState 和
//! agent/events.rs 里，Context 是给未来 skill 用的统一入口。

use tauri::Window;

use crate::state::{AppState, ConfirmDecision, PendingConfirmation};
use tokio::sync::oneshot;

/// Skill 的 Tool 在 `call()` 里访问基座能力的上下文。
#[derive(Clone)]
#[allow(dead_code)]
pub struct SkillContext {
    pub app_state: AppState,
    pub task_id: String,
    pub window: Window,
}

#[allow(dead_code)]
impl SkillContext {
    pub fn new(app_state: AppState, task_id: String, window: Window) -> Self {
        Self { app_state, task_id, window }
    }

    /// 挂起当前操作等待用户确认（变更前确认模式）。返回 true=用户批准，false=取消。
    /// key 格式：`{task_id}:{tool_call_id}`。
    pub async fn await_confirmation(
        &self,
        tool_call_id: &str,
        summary: String,
        detail: String,
    ) -> bool {
        use crate::agent::events::emit_tool_awaiting;

        let (tx, rx) = oneshot::channel::<ConfirmDecision>();
        {
            let key = format!("{}:{}", self.task_id, tool_call_id);
            let mut pending = self.app_state.pending_confirmations.lock().await;
            pending.insert(key, PendingConfirmation { tx });
        }
        emit_tool_awaiting(&self.window, &self.task_id, tool_call_id, summary, detail);

        match rx.await {
            Ok(d) => d.approved,
            Err(_) => false,
        }
    }

    /// 检查当前 task 是否被中止。
    pub async fn is_aborted(&self) -> bool {
        let aborted = self.app_state.aborted_tasks.lock().await;
        aborted.contains(&self.task_id)
    }
}
