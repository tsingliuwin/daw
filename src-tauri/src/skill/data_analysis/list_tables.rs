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
            // 先查出所有 ATTACH 的外部数据源 catalog（db_ 前缀），
            // 再逐个 USE + SHOW TABLES 列出表（走 DuckDB 原生命令，
            // 不触发 postgres 扩展的元数据扫描，兼容 Hologres 等）。
            let catalogs: Vec<String> = guard
                .prepare("SELECT DISTINCT database_name FROM duckdb_databases() WHERE database_name LIKE 'db_%'")
                .map_err(|e| e.to_string())?
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();

            let mut list = Vec::new();
            for catalog in &catalogs {
                if let Err(e) = guard.execute_batch(&format!("USE {}", catalog)) {
                    tracing::warn!(category = "query", "USE {} 失败: {}", catalog, e);
                    continue;
                }
                match guard.prepare("SHOW TABLES") {
                    Ok(mut stmt) => {
                        if let Ok(rows) = stmt.query_map([], |r| {
                            let name: String = r.get(0)?;
                            let parts: Vec<&str> = name.splitn(2, '.').collect();
                            let schema = if parts.len() == 2 { parts[0].to_string() } else { "main".to_string() };
                            let table = if parts.len() == 2 { parts[1].to_string() } else { parts[0].to_string() };
                            Ok((schema, table))
                        }) {
                            for r in rows.flatten() {
                                list.push((catalog.clone(), r.0, r.1));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(category = "query", "SHOW TABLES in {} 失败: {}", catalog, e);
                    }
                }
            }
            // 切回默认 catalog。
            let _ = guard.execute_batch("USE memory");
            list.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
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
