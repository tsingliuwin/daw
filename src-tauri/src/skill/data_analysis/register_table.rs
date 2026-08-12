use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::attach::workspace_attach_alias;
use super::super::super::model::TableRegistryEntry;
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
        let conn_name = args.connection_name.trim().to_string();
        let table_name = args.table_name.trim().to_string();
        let local = args.local_name.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
        let local_name = local.map(|s| s.to_string()).unwrap_or_else(|| {
            let short = table_name.rsplit('.').next().unwrap_or(&table_name);
            format!("v_{short}")
        });

        for part in [&local_name, &conn_name, &table_name] {
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

        let catalog = workspace_attach_alias(&conn_name);
        let db_type = conn_record.db_type.clone();
        let db_product = conn_record.db_product.clone();
        let db_mode = conn_record.db_mode.clone();
        let local_name_clone = local_name.clone();
        let table_name_clone = table_name.clone();
        let catalog_clone = catalog.clone();
        let ws_path_clone = ws_path.clone();
        let conn_name_clone = conn_name.clone();
        let conn_name_for_registry = conn_name.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<(String, String, String), String> {
            let guard = duckdb_conn.blocking_lock();

            // 解析 schema.table
            let parts: Vec<&str> = table_name_clone.splitn(2, '.').collect();
            if parts.len() != 2 {
                return Err(format!("table_name 必须包含 schema，格式为 schema.table（如 default.orders）。"));
            }
            let (schema, tbl) = (parts[0], parts[1]);

            // 检测 table_type（native vs foreign）
            let table_type = if db_type == "postgres" {
                let check_sql = format!(
                    "SELECT * FROM postgres_query('{}', 'SELECT count(*) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = ''f'' AND n.nspname = ''{}'' AND c.relname = ''{}''')",
                    catalog_clone, schema, tbl
                );
                let is_foreign: i64 = guard.query_row(&check_sql, [], |r| r.get(0)).unwrap_or(0);
                if is_foreign > 0 { "foreign" } else { "native" }
            } else {
                "native"
            };

            // 判断 access_mode
            let access_mode = if db_mode == "external" {
                // Hologres 外部库：catalog 路径不可用（pg_namespace 扫描报错）
                "pushdown"
            } else if db_mode == "standard" {
                // 标准库：尝试 catalog 路径
                let test_sql = format!("SELECT * FROM {}.{}.\"{}\" LIMIT 0", catalog_clone, schema, tbl);
                match guard.prepare(&test_sql) {
                    Ok(_) => "catalog",
                    Err(_) => "pushdown",
                }
            } else {
                // unknown：尝试 catalog
                let test_sql = format!("SELECT * FROM {}.{}.\"{}\" LIMIT 0", catalog_clone, schema, tbl);
                match guard.prepare(&test_sql) {
                    Ok(_) => "catalog",
                    Err(_) => "pushdown",
                }
            };

            // 根据 access_mode 创建视图或记录 pushdown 引用
            let remote_ref = if access_mode == "catalog" {
                // catalog 模式：创建视图引用远程 catalog 表
                format!("{}.{}", catalog_clone, table_name_clone)
            } else {
                // pushdown 模式：用 postgres_query 创建视图
                let inner = format!("SELECT * FROM \"{}\".\"{}\"", schema, tbl);
                let inner_escaped = inner.replace('\'', "''");
                let pushdown = format!("postgres_query('{}', '{}')", catalog_clone, inner_escaped);
                // 测试 pushdown 能否解析
                let test_sql = format!("SELECT * FROM {} LIMIT 0", pushdown);
                match guard.prepare(&test_sql) {
                    Ok(_) => pushdown,
                    Err(e1) => {
                        let err1 = e1.to_string();
                        tracing::warn!(category = "query", "pushdown SELECT * 失败，尝试 CAST: {}", err1);
                        let mut cast_cols = Vec::new();
                        let mut remaining_err = err1;
                        for _ in 0..5 {
                            if let Some(col) = extract_unsupported_column(&remaining_err) {
                                cast_cols.push(col.clone());
                                let cast_sql = build_cast_sql(&catalog_clone, schema, tbl, &cast_cols);
                                let test2 = format!("SELECT * FROM {} LIMIT 0", cast_sql);
                                match guard.prepare(&test2) {
                                    Ok(_) => return Ok((cast_sql, table_type.to_string(), access_mode.to_string())),
                                    Err(e2) => { remaining_err = e2.to_string(); }
                                }
                            } else { break; }
                        }
                        format!("postgres_query('{}', 'SELECT * FROM \"{}\".\"{}\"')", catalog_clone, schema, tbl)
                    }
                }
            };

            // CREATE OR REPLACE 视图
            let sql = format!("CREATE OR REPLACE VIEW \"{}\" AS SELECT * FROM {};", local_name_clone, remote_ref);
            guard.execute_batch(&sql).map_err(|e| {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                if lower.contains("permission denied") || lower.contains("access denied") || lower.contains("does not exist") {
                    format!("注册失败，当前用户可能没有表 {table_name_clone} 的查询权限，或表不存在。错误: {msg}\n建议：跳过此表，用 list_remote_tables 查看其他可用表。")
                } else {
                    format!("注册失败: {msg}")
                }
            })?;

            // 写入 table_registry
            let entry = TableRegistryEntry {
                id: format!("tr-{}-{}", now_ms(), local_name_clone),
                workspace_path: ws_path_clone,
                connection_name: conn_name_for_registry.clone(),
                local_name: local_name_clone.clone(),
                remote_schema: schema.to_string(),
                remote_table: tbl.to_string(),
                db_type: db_type.clone(),
                db_product: db_product.clone(),
                db_mode: db_mode.clone(),
                table_type: table_type.to_string(),
                access_mode: access_mode.to_string(),
                status: "available".to_string(),
                unavailable_reason: None,
                last_explored: Some(now_ms()),
            };
            let _ = crate::db::upsert_table_registry(&entry);

            Ok((remote_ref, table_type.to_string(), access_mode.to_string()))
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok((remote_ref, table_type, access_mode)) => {
                let summary = format!("已注册 {} ({}={}，{}={})", local_name, "access", access_mode, "type", table_type);
                let detail = format!("CREATE OR REPLACE VIEW \"{}\" AS SELECT * FROM {};", local_name, remote_ref);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), Some(detail), None, Some(elapsed), None);
                if access_mode == "pushdown" {
                    Ok(format!("{summary}。此表为 pushdown 模式，查询时请用 SELECT * FROM postgres_query('db_{}', 'SELECT ... FROM \"{}\" ...') 下推。", conn_name, table_name))
                } else {
                    Ok(format!("{summary}。后续可用 {} 作为表名查询。", local_name))
                }
            }
            Err(err) => {
                let err_msg = err.0.clone();
                let (status, reason) = classify_error(&err_msg);
                // 更新 table_registry 状态（如果记录存在）
                let ws_path_clone2 = ws_path.clone();
                let local_name_clone2 = local_name.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::db::update_table_registry_status(&ws_path_clone2, &local_name_clone2, &status, Some(&reason));
                }).await;
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), None, None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 从 DuckDB 错误信息中提取不支持类型的列名。
fn extract_unsupported_column(err: &str) -> Option<String> {
    let marker = "unsupported type in column ";
    if let Some(pos) = err.find(marker) {
        let rest = &err[pos + marker.len()..];
        let end = rest.find(|c: char| c == '.' || c == ':' || c == '\n').unwrap_or(rest.len());
        let col = rest[..end].trim().trim_end_matches('.').to_string();
        if !col.is_empty() { return Some(col); }
    }
    None
}

fn build_cast_sql(catalog: &str, schema: &str, tbl: &str, cast_cols: &[String]) -> String {
    let cast_list = cast_cols.iter().map(|c| format!("CAST(\"{}\" AS TEXT) AS \"{}\"", c, c)).collect::<Vec<_>>().join(", ");
    let inner = format!("SELECT * FROM \"{}\".\"{}\"", schema, tbl);
    let inner_escaped = inner.replace('\'', "''");
    format!("SELECT * REPLACE ({}) FROM postgres_query('{}', '{}')", cast_list, catalog, inner_escaped)
}

fn classify_error(err: &str) -> (String, String) {
    let lower = err.to_lowercase();
    if lower.contains("storage tier") || lower.contains("lower meta") || lower.contains("odps") {
        ("unavailable_permanent".to_string(), "MaxCompute 非标准存储，Hologres 不支持访问".to_string())
    } else if lower.contains("unsupported type") {
        ("unavailable_permanent".to_string(), "列类型不兼容".to_string())
    } else if lower.contains("permission") || lower.contains("privilege") || lower.contains("authorize") || lower.contains("deny") {
        ("unavailable_temporary".to_string(), "权限不足".to_string())
    } else if lower.contains("does not exist") || lower.contains("not found") {
        ("unavailable_temporary".to_string(), "表不存在".to_string())
    } else if lower.contains("timeout") || lower.contains("connection") {
        ("unavailable_temporary".to_string(), "连接超时".to_string())
    } else {
        ("unavailable_temporary".to_string(), err.to_string())
    }
}
