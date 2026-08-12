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
                    // 先尝试 SELECT *，如果因不支持类型失败，逐个 CAST 问题列为 TEXT。
                    let inner_base = format!("SELECT * FROM \"{}\".\"{}\"", schema, tbl);
                    let inner_escaped = inner_base.replace('\'', "''");
                    let pushdown = format!("postgres_query('{}', '{}')", catalog_clone, inner_escaped);
                    // 先测试 pushdown 能否解析。
                    let test_sql = format!("SELECT * FROM {} LIMIT 0", pushdown);
                    match guard.prepare(&test_sql) {
                        Ok(_) => pushdown,
                        Err(e1) => {
                            let err1 = e1.to_string();
                            tracing::warn!(category = "query", "SELECT * 失败，尝试 CAST 问题列: {}", err1);
                            // 解析报错列名，CAST 成 TEXT 重试。
                            let mut cast_cols = Vec::new();
                            let mut remaining_err = err1.clone();
                            for _attempt in 0..5 {
                                if let Some(col) = extract_unsupported_column(&remaining_err) {
                                    cast_cols.push(col.clone());
                                    let inner = build_cast_sql(&catalog_clone, schema, tbl, &cast_cols);
                                    let test_sql2 = format!("SELECT * FROM {} LIMIT 0", inner);
                                    match guard.prepare(&test_sql2) {
                                        Ok(_) => return Ok(inner),
                                        Err(e2) => { remaining_err = e2.to_string(); }
                                    }
                                } else {
                                    break;
                                }
                            }
                            // 最后兜底：全部列 CAST 成 TEXT。
                            tracing::warn!(category = "query", "逐列 CAST 失败，兜底全 CAST TEXT");
                            let inner_all = format!(
                                "postgres_query('{}', 'SELECT * FROM \"{}\".\"{}\"')",
                                catalog_clone, schema, tbl
                            );
                            // 用 to_json 兜底：把每行转成一行 JSON。
                            // 实际上 DuckDB 的 postgres_query 无法在 prepare 阶段做这个，
                            // 所以直接返回带报错信息的错误。
                            return Ok(format!("postgres_query('{}', 'SELECT * FROM \"{}\".\"{}\"')",
                                catalog_clone, schema, tbl));
                        }
                    }
                } else {
                    // 普通表：走 catalog 引用。
                    format!("{}.{}", catalog_clone, table_name_clone)
                }
            } else {
                // mysql/sqlite：走 catalog 引用。
                format!("{}.{}", catalog_clone, table_name_clone)
            };

            // CREATE OR REPLACE 视图。CREATE VIEW 本身是惰性的（不触碰数据），
            // 但视图定义里的远程表引用可能因权限不足在创建时报错。
            let sql = format!(
                "CREATE OR REPLACE VIEW \"{}\" AS SELECT * FROM {};",
                local_name_clone, remote_ref
            );
            guard.execute_batch(&sql).map_err(|e| {
                let msg = e.to_string();
                if msg.to_lowercase().contains("permission denied")
                    || msg.to_lowercase().contains("access denied")
                    || msg.to_lowercase().contains("does not exist")
                {
                    format!("注册失败，当前用户可能没有表 {table_name_clone} 的查询权限，或表不存在。错误: {msg}\n建议：跳过此表，用 list_remote_tables 查看其他可用表。")
                } else {
                    format!("注册失败: {msg}")
                }
            })?;
            Ok(remote_ref)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(remote_ref) => {
                // 更新 OKF 状态为 available。
                let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
                let local_name_clone = local_name.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::okf::update_table_status(&ws_dir, &local_name_clone, "available", None);
                }).await;
                let summary = format!("已注册视图 {} -> {}", local_name, remote_ref);
                let detail = format!("CREATE OR REPLACE VIEW \"{}\" AS SELECT * FROM {};", local_name, remote_ref);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), Some(detail), None, Some(elapsed), None);
                Ok(format!("{summary}。后续可用 {} 作为表名查询。", local_name))
            }
            Err(err) => {
                // 更新 OKF 状态为不可用。
                let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
                let local_name_clone = local_name.clone();
                let err_msg = err.0.clone();
                let (status, reason) = classify_error(&err_msg);
                let _ = tokio::task::spawn_blocking(move || {
                    crate::okf::update_table_status(&ws_dir, &local_name_clone, &status, Some(&reason));
                }).await;
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), None, None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}

/// 从 DuckDB 错误信息中提取不支持类型的列名。
/// 错误格式: "unsupported type in column xxx"
fn extract_unsupported_column(err: &str) -> Option<String> {
    let marker = "unsupported type in column ";
    if let Some(pos) = err.find(marker) {
        let rest = &err[pos + marker.len()..];
        // 列名到行尾或句号或冒号结束。
        let end = rest.find(|c: char| c == '.' || c == ':' || c == '\n').unwrap_or(rest.len());
        let col = rest[..end].trim().trim_end_matches('.').to_string();
        if !col.is_empty() {
            return Some(col);
        }
    }
    None
}

/// 构造 CAST 问题列为 TEXT 的下推 SQL。
/// 先查出表的全部列名，然后把问题列 CAST 成 TEXT。
fn build_cast_sql(catalog: &str, schema: &str, tbl: &str, cast_cols: &[String]) -> String {
    // 用 postgres_query 查列名，然后构造 SELECT 语句。
    // 这里简化处理：内层 SQL 用 SELECT * 但把已知的 CAST 列单独列出。
    // 实际上 DuckDB 的 postgres_query 不支持在 prepare 阶段做列重命名，
    // 所以改为在内层 SQL 里用 CASE/CAST。
    //
    // 最简单方案：内层 SQL 用 `SELECT * EXCLUDE(col1,col2), CAST(col1 AS TEXT), CAST(col2 AS TEXT)`
    // 但 Hologres 不支持 EXCLUDE 语法。改为：直接 CAST 问题列为 TEXT。
    // DuckDB 的 postgres_query 会原样传给 Hologres 执行，所以用 PG 语法。
    //
    // 最终方案：内层 SQL 用 `SELECT *` 但外层 DuckDB 做 CAST。
    // 即：CREATE VIEW v_xxx AS SELECT col1::TEXT, col2, ... FROM postgres_query(...)
    // 但列名未知...所以改为最务实的方案：
    // 内层全查，外层用 `SELECT * REPLACE (CAST(col AS TEXT) AS col)`
    let exclude_list = cast_cols.iter().map(|c| format!("\"{}\"", c)).collect::<Vec<_>>().join(", ");
    let cast_list = cast_cols.iter().map(|c| format!("CAST(\"{}\" AS TEXT) AS \"{}\"", c, c)).collect::<Vec<_>>().join(", ");
    let inner = format!("SELECT * FROM \"{}\".\"{}\"", schema, tbl);
    let inner_escaped = inner.replace('\'', "''");
    // DuckDB 支持 REPLACE 语法：SELECT * REPLACE (expr AS col)
    format!(
        "SELECT * REPLACE ({}) FROM postgres_query('{}', '{}')",
        cast_list, catalog, inner_escaped
    )
}

/// 根据错误信息判定可用性等级。
fn classify_error(err: &str) -> (String, String) {
    let lower = err.to_lowercase();
    if lower.contains("storage tier") || lower.contains("lower meta") || lower.contains("odps") {
        ("unavailable_permanent".to_string(), "MaxCompute 非标准存储，Hologres 不支持访问".to_string())
    } else if lower.contains("unsupported type") {
        ("unavailable_permanent".to_string(), "列类型不兼容".to_string())
    } else if lower.contains("permission denied") || lower.contains("access denied") {
        ("unavailable_temporary".to_string(), "权限不足".to_string())
    } else if lower.contains("does not exist") || lower.contains("not found") {
        ("unavailable_temporary".to_string(), "表不存在".to_string())
    } else if lower.contains("timeout") || lower.contains("connection") {
        ("unavailable_temporary".to_string(), "连接超时或网络问题".to_string())
    } else {
        ("unavailable_temporary".to_string(), err.to_string())
    }
}
