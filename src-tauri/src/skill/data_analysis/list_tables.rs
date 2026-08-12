use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ListTablesArgs {}

pub struct ListTablesTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for ListTablesTool {
    const NAME: &'static str = "list_tables";
    type Error = ToolError;
    type Args = ListTablesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_tables".to_string(),
            description: "列出已注册的表/视图及其状态。远程表通过 list_remote_tables + register_table 注册后才出现。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("list");
        emit_tool_call(&self.window, &self.task_id, &call_id, "list_tables", json!({}));

        let start = std::time::Instant::now();

        // 从 table_registry 读取已注册的表。
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let entries = {
            let ws = ws_path.clone();
            tokio::task::spawn_blocking(move || crate::db::list_table_registry(&ws))
                .await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
                .map_err(|e| ToolError(format!("数据库查询失败: {e}")))?
        };

        let elapsed = start.elapsed().as_millis() as u64;

        // 格式化输出
        let mut lines: Vec<String> = Vec::new();
        for e in &entries {
            let icon = match e.status.as_str() {
                "available" => "✅",
                "unavailable_permanent" => "❌",
                "unavailable_temporary" => "⚠️",
                _ => "❓",
            };
            let mode = if e.access_mode == "pushdown" { " [pushdown]" } else { "" };
            let reason = if e.status != "available" && e.unavailable_reason.is_some() {
                format!(" — {}", e.unavailable_reason.as_ref().unwrap())
            } else { String::new() };
            lines.push(format!("{icon} {} ({}){}{}", e.local_name, e.connection_name, mode, reason));
        }

        let summary = if lines.is_empty() {
            "当前没有已注册的表。请先调用 list_connections 和 list_remote_tables 发现并注册表。".to_string()
        } else {
            format!("已注册 {} 个表/视图", entries.len())
        };
        let result = if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        };
        emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, None, None, Some(elapsed), result);
        Ok(if lines.is_empty() {
            "当前没有已注册的表/视图。".to_string()
        } else {
            format!("已注册的表/视图:\n{}", lines.join("\n"))
        })
    }
}
