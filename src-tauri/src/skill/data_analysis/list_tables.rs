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
            description: "列出当前已连接的所有数据库中的表和视图。开始探索前应先调用此工具了解有哪些数据。每个表名前缀标注了它所属的数据源（如 db_myshop.public.orders）。".to_string(),
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

        let conn = match &self.app_state.duckdb {
            Some(c) => c.clone(),
            None => {
                let msg = "DuckDB 引擎未初始化。请检查是否配置了数据源并重启应用。".to_string();
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    msg.clone(), None, None, Some(0), None,
                );
                return Err(ToolError(msg));
            }
        };

        let tables_res = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
            let guard = conn.blocking_lock();
            // 查当前 catalog 的表和视图（已注册的视图 + 本地表）。
            // 用 information_schema.tables 限制 table_catalog 排除 db_ 前缀的远程 catalog，
            // 避免触发 postgres 扩展的元数据扫描。
            let sql = "
                SELECT table_name FROM information_schema.tables
                WHERE table_schema = 'main'
                AND table_catalog NOT LIKE 'db_%'
                ORDER BY table_name
            ";
            let mut stmt = guard.prepare(sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            let mut list = Vec::new();
            for r in rows {
                list.push(r.map_err(|e| e.to_string())?);
            }
            Ok(list)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("数据库查询失败: {e}"))));

        let elapsed = start.elapsed().as_millis() as u64;
        match tables_res {
            Ok(tables) => {
                let summary = if tables.is_empty() {
                    "当前没有已注册的表。请先调用 list_connections 和 list_remote_tables 发现并注册表。".to_string()
                } else {
                    format!("已注册 {} 个表/视图: {}", tables.len(), tables.join(", "))
                };
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, None, Some(elapsed), None,
                );
                Ok(format!("当前已注册的表/视图: {}", tables.join("; ")))
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}
