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

        let tables_res = tokio::task::spawn_blocking(move || -> Result<Vec<(String, String, String)>, String> {
            let guard = conn.blocking_lock();
            // 列出所有非 internal 的表和视图（不限 schema，否则 postgres/mysql 不可见）。
            // 过滤掉 DuckDB 内部 catalog（memory/system/temp），只保留 db_ 前缀的外部数据源。
            let sql = "
                SELECT table_catalog, schema_name, table_name FROM duckdb_tables() WHERE NOT internal
                UNION
                SELECT table_catalog, schema_name, view_name AS table_name FROM duckdb_views() WHERE NOT internal
                ORDER BY table_catalog, schema_name, table_name
            ";
            let mut stmt = guard.prepare(sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                .map_err(|e| e.to_string())?;
            let mut list = Vec::new();
            for r in rows {
                let (catalog, schema, name) = r.map_err(|e| e.to_string())?;
                // 只保留 ATTACH 的外部数据源（db_ 前缀）。
                if catalog.starts_with("db_") {
                    list.push((catalog, schema, name));
                }
            }
            Ok(list)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("数据库查询失败: {e}"))));

        let elapsed = start.elapsed().as_millis() as u64;
        match tables_res {
            Ok(tables) => {
                // 列出每个表的三段式全限定名（catalog.schema.table）。
                let full_names: Vec<String> = tables.iter()
                    .map(|(catalog, schema, name)| format!("{catalog}.{schema}.{name}"))
                    .collect();
                let summary = if full_names.is_empty() {
                    "当前没有找到任何表。请在设置中配置并启用数据源。".to_string()
                } else {
                    format!("探测到 {} 张表: {}", full_names.len(), full_names.join(", "))
                };
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, None, Some(elapsed), None,
                );
                Ok(format!("当前可用的数据库表列表为: {}", full_names.join("; ")))
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
