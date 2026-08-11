use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::attach::{build_pg_conn_str, workspace_attach_alias};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ListRemoteTablesArgs {
    connection_name: String,
}

pub struct ListRemoteTablesTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for ListRemoteTablesTool {
    const NAME: &'static str = "list_remote_tables";
    type Error = ToolError;
    type Args = ListRemoteTablesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_remote_tables".to_string(),
            description: "探查指定数据源连接下有哪些表和视图。返回 schema.table 格式的表名（如 public.orders）。用于从数据源中发现与用户分析目标相关的表。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "connection_name": { "type": "string", "description": "数据源名称（如 myshop）" }
                },
                "required": ["connection_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("rt");
        let conn_name = args.connection_name.trim();
        emit_tool_call(&self.window, &self.task_id, &call_id, "list_remote_tables", json!({
            "connection_name": conn_name,
        }));

        let start = std::time::Instant::now();

        // 从 SQLite 查连接信息。
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let conn_record = {
            let ws_path = ws_path.clone();
            let name = conn_name.to_string();
            tokio::task::spawn_blocking(move || {
                crate::db::get_workspace_db_connection_by_name(&ws_path, &name)
            }).await
            .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
            .map_err(|e| ToolError(e))?
        };

        let conn_record = match conn_record {
            Some(c) => c,
            None => {
                let msg = format!("数据源 {} 不存在或未启用。", conn_name);
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), None, None, Some(0), None);
                return Err(ToolError(msg));
            }
        };

        let duckdb_conn = match &self.app_state.duckdb {
            Some(c) => c.clone(),
            None => {
                let msg = "DuckDB 引擎未初始化。".to_string();
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), None, None, Some(0), None);
                return Err(ToolError(msg));
            }
        };

        let db_type = conn_record.db_type.clone();
        let catalog = workspace_attach_alias(&conn_record.name);
        let tables_res = tokio::task::spawn_blocking(move || -> Result<Vec<(String, String)>, String> {
            let guard = duckdb_conn.blocking_lock();

            if db_type == "postgres" {
                // postgres 类型：用 postgres_query 下推查 information_schema，
                // 传 catalog 别名（db_xxx）而非连接串——postgres_query 用已 ATTACH 的
                // catalog 别名解析连接，不重新初始化 catalog（避免元数据扫描）。
                // 内层 SQL 的单引号转义成 '' 避免和外层引号冲突。
                let inner_sql = "SELECT table_schema, table_name FROM information_schema.tables \
                    WHERE table_schema NOT IN (''pg_catalog'', ''information_schema'') \
                    ORDER BY table_schema, table_name";
                let sql = format!(
                    "SELECT * FROM postgres_query('{}', '{}')",
                    catalog, inner_sql
                );
                let mut stmt = guard.prepare(&sql).map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .map_err(|e| e.to_string())?;
                let mut list = Vec::new();
                for r in rows {
                    list.push(r.map_err(|e| e.to_string())?);
                }
                Ok(list)
            } else {
                // mysql/sqlite：走 DuckDB catalog（这些类型的元数据扫描不报错）。
                let catalog = workspace_attach_alias(&conn_record.name);
                guard.execute_batch(&format!("USE {}", catalog)).map_err(|e| e.to_string())?;
                let mut stmt = guard.prepare("SHOW TABLES").map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |r| {
                        let name: String = r.get(0)?;
                        let parts: Vec<&str> = name.splitn(2, '.').collect();
                        let schema = if parts.len() == 2 { parts[0].to_string() } else { "main".to_string() };
                        let table = if parts.len() == 2 { parts[1].to_string() } else { parts[0].to_string() };
                        Ok((schema, table))
                    })
                    .map_err(|e| e.to_string())?;
                let mut list = Vec::new();
                for r in rows {
                    list.push(r.map_err(|e| e.to_string())?);
                }
                let _ = guard.execute_batch("USE memory");
                Ok(list)
            }
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match tables_res {
            Ok(tables) => {
                let full_names: Vec<String> = tables.iter()
                    .map(|(schema, name)| format!("{schema}.{name}"))
                    .collect();
                let summary = if full_names.is_empty() {
                    format!("数据源 {} 中没有找到任何表。", conn_name)
                } else {
                    format!("数据源 {} 中有 {} 张表", conn_name, full_names.len())
                };
                let out = if full_names.is_empty() {
                    summary.clone()
                } else {
                    format!("数据源 {} 中的表：\n{}", conn_name, full_names.iter().map(|n| format!("- {n}")).collect::<Vec<_>>().join("\n"))
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
