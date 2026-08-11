use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ListConnectionsArgs {}

pub struct ListConnectionsTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for ListConnectionsTool {
    const NAME: &'static str = "list_connections";
    type Error = ToolError;
    type Args = ListConnectionsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_connections".to_string(),
            description: "列出当前工作区已连接的所有数据源。开始分析前应先调用此工具了解有哪些数据源可用。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("conn");
        emit_tool_call(&self.window, &self.task_id, &call_id, "list_connections", json!({}));

        let start = std::time::Instant::now();
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        tracing::info!(category = "agent", "list_connections: workspace_path={}", ws_path);
        let result = tokio::task::spawn_blocking(move || -> Result<Vec<crate::model::DataSourceConfig>, String> {
            crate::db::list_workspace_db_connections(&ws_path).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(conns) => {
                let lines: Vec<String> = conns.iter().enumerate().map(|(i, c)| {
                    let summary = if c.db_type == "sqlite" {
                        c.database_name.clone()
                    } else {
                        format!("{}:{}/{}", c.host, c.port, c.database_name)
                    };
                    format!("{}. {} ({}) — {}", i + 1, c.name, c.db_type, summary)
                }).collect();
                let summary = if conns.is_empty() {
                    "当前工作区没有连接任何数据源。请在设置中配置并启用数据源。".to_string()
                } else {
                    format!("已连接 {} 个数据源", conns.len())
                };
                let out = if conns.is_empty() {
                    summary.clone()
                } else {
                    format!("当前已连接的数据源（{} 个）：\n{}", conns.len(), lines.join("\n"))
                };
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, None, None, Some(elapsed), Some(out.clone()));
                Ok(out)
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), None, None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}
