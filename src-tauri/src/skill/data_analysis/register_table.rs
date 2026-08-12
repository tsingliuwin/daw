use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::attach::{build_pg_conn_str, workspace_attach_alias};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct RegisterTableArgs {
    connection_name: String,
    table_name: String,
    #[serde(default)]
    local_name: Option<String>,
}

pub struct RegisterTableTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for RegisterTableTool {
    const NAME: &'static str = "register_table";
    type Error = ToolError;
    type Args = RegisterTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "register_table".to_string(),
            description: "将远程数据源中的表注册为本地视图（短名映射）。注册后可用短名直接查询、describe、sample，无需写三段式全限定名。选择与用户分析目标相关的表进行注册。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "connection_name": { "type": "string", "description": "数据源名称（如 myshop）" },
                    "table_name": { "type": "string", "description": "远程表名，必须包含 schema（如 default.orders 或 public.orders），从 list_remote_tables 的返回结果中复制完整 schema.table" },
                    "local_name": { "type": "string", "description": "本地视图名（可选，默认 v_{table名}）。注册后用此名查询" }
                },
                "required": ["connection_name", "table_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let conn_name = args.connection_name.trim();
        let table_name = args.table_name.trim();
        let local = args.local_name.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
        let local_name = local.map(|s| s.to_string()).unwrap_or_else(|| {
            let short = table_name.rsplit('.').next().unwrap_or(table_name);
            format!("v_{short}")
        });

        for part in [&local_name, conn_name, table_name] {
            if part.is_empty() || part.contains('"') || part.contains('\0') {
                return Err(ToolError(format!("非法名称: {part:?}")));
            }
        }

        let call_id = next_tool_id("reg");
        emit_tool_call(&self.window, &self.task_id, &call_id, "register_table", json!({
            "connection_name": conn_name,
            "table_name": table_name,
            "local_name": &local_name,
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

        let duckdb_guard = self.app_state.duckdb.lock().await;
        let duckdb_conn = match &*duckdb_guard {
            Some(c) => c.clone(),
            None => {
                let msg = "DuckDB 引擎未初始化。".to_string();
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), None, None, Some(0), None);
                return Err(ToolError(msg));
            }
        };

        let catalog = workspace_attach_alias(conn_name);
        let db_type = conn_record.db_type.clone();
        let local_name_clone = local_name.clone();
        let table_name_clone = table_name.to_string();
        let catalog_clone = catalog.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let guard = duckdb_conn.blocking_lock();

            // 构造视图的远程引用路径。
            let remote_ref = if db_type == "postgres" {
                // postgres 类型：先检测是不是 foreign table（Hologres MaxCompute 外表）。
                // 用 catalog 别名调 postgres_query（和 lakemind 一致）。
                let parts: Vec<&str> = table_name_clone.splitn(2, '.').collect();
                if parts.len() != 2 {
                    return Err(format!("table_name 必须包含 schema，格式为 schema.table（如 default.orders）。请从 list_remote_tables 的结果中获取完整名称。"));
                }
                let (schema, tbl) = (parts[0], parts[1]);
                let check_sql = format!(
                    "SELECT count(*) FROM pg_catalog.pg_class c \
                     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE c.relkind = ''f'' AND n.nspname = ''{}'' AND c.relname = ''{}''",
                    schema, tbl
                );
                let sql = format!("SELECT * FROM postgres_query('{}', '{}')", catalog_clone, check_sql);

                let is_foreign: i64 = guard.query_row(&sql, [], |r| r.get(0)).unwrap_or(0);

                if is_foreign > 0 {
                    // foreign table：用 postgres_query 下推创建视图。
                    let inner = format!("SELECT * FROM \"{}\".\"{}\"", schema, tbl);
                    let inner_escaped = inner.replace('\'', "''");
                    format!("postgres_query('{}', '{}')", catalog_clone, inner_escaped)
                } else {
                    // 普通表：走 catalog 引用。
                    format!("{}.{}", catalog_clone, table_name_clone)
                }
            } else {
                // mysql/sqlite：走 catalog 引用。
                format!("{}.{}", catalog_clone, table_name_clone)
            };

            // CREATE OR REPLACE 视图。
            let sql = format!(
                "CREATE OR REPLACE VIEW \"{}\" AS SELECT * FROM {};",
                local_name_clone, remote_ref
            );
            guard.execute_batch(&sql).map_err(|e| e.to_string())?;
            Ok(remote_ref)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(remote_ref) => {
                let summary = format!("已注册视图 {} -> {}", local_name, remote_ref);
                let detail = format!("CREATE OR REPLACE VIEW \"{}\" AS SELECT * FROM {};", local_name, remote_ref);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), Some(detail), None, Some(elapsed), None);
                Ok(format!("{summary}。后续可用 {} 作为表名查询。", local_name))
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), None, None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}
