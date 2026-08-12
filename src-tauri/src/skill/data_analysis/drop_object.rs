use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};
use tokio::sync::oneshot;

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_awaiting, emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct DropObjectArgs {
    name: String,
}

pub struct DropObjectTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub confirm_mode: String,
}

impl Tool for DropObjectTool {
    const NAME: &'static str = "drop_object";
    type Error = ToolError;
    type Args = DropObjectArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "drop_object".to_string(),
            description: "删除指定的视图或表。仅当用户明确要求删除时使用。删除前会请求用户确认。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "要删除的视图或表名" }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let name = args.name.trim();
        if name.is_empty() || name.contains('"') || name.contains('\0') {
            return Err(ToolError(format!("非法对象名: {name:?}")));
        }

        let call_id = next_tool_id("drop");
        emit_tool_call(&self.window, &self.task_id, &call_id, "drop_object", json!({
            "name": name,
        }));

        let start = std::time::Instant::now();
        let summary_pending = format!("删除对象 {}", name);
        // 双保险 DDL：同时 DROP VIEW 和 DROP TABLE（IF EXISTS 确保不存在的类型不报错）。
        let ddl = format!(
            "DROP VIEW IF EXISTS \"{name}\";\nDROP TABLE IF EXISTS \"{name}\";",
        );

        // 确认 gate：仅"变更前确认"模式才挂起等待用户确认。
        let approved = if self.confirm_mode != "变更前确认" {
            true
        } else {
            let (tx, rx) = oneshot::channel::<crate::state::ConfirmDecision>();
            {
                let key = format!("{}:{}", self.task_id, call_id);
                let mut pending = self.app_state.pending_confirmations.lock().await;
                pending.insert(key, crate::state::PendingConfirmation { tx });
            }
            emit_tool_awaiting(&self.window, &self.task_id, &call_id, summary_pending.clone(), ddl.clone());
            match rx.await {
                Ok(d) => d.approved,
                Err(_) => false,
            }
        };

        if !approved {
            let msg = "用户已取消此操作".to_string();
            emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), Some(ddl), None, Some(start.elapsed().as_millis() as u64), None);
            return Err(ToolError(msg));
        }

        // 执行。
        let duckdb_guard = self.app_state.duckdb.lock().await;
        let conn = match &*duckdb_guard {
            Some(c) => c.clone(),
            None => {
                let msg = "DuckDB 引擎未初始化".to_string();
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), None, None, Some(0), None);
                return Err(ToolError(msg));
            }
        };
        let ddl_clone = ddl.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let guard = conn.blocking_lock();
            guard.execute_batch(&ddl_clone).map_err(|e| e.to_string())
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(()) => {
                // 删 OKF 文件。
                let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
                let name_clone = name.to_string();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::okf::delete_okf_file(&ws_dir, &name_clone);
                }).await;

                let summary = format!("对象 {} 已删除", name);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), Some(ddl), None, Some(elapsed), None);
                Ok(summary)
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), Some(ddl), None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}
