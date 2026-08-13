use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::{execute, QUERY_HARD_TIMEOUT_SECS};
use super::super::super::model::SqlResult;
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ExecuteQueryArgs {
    sql: String,
}

pub struct ExecuteQueryTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for ExecuteQueryTool {
    const NAME: &'static str = "execute_query";
    type Error = ToolError;
    type Args = ExecuteQueryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "execute_query".to_string(),
            description: "执行只读的 SQL 查询，并返回结果。只允许 SELECT，禁止 DROP/ALTER/UPDATE/DELETE/INSERT 等。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "要执行的 SQL 查询语句" }
                },
                "required": ["sql"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let sql = args.sql.trim();
        let sql_upper = sql.to_uppercase();
        let forbidden_keywords = ["DROP", "DELETE", "UPDATE", "INSERT", "ALTER", "TRUNCATE", "ATTACH", "DETACH"];
        for keyword in &forbidden_keywords {
            if sql_upper.contains(keyword) {
                return Err(ToolError(format!("出于安全考虑，禁止执行包含 {} 操作的 SQL 语句。", keyword)));
            }
        }

        let call_id = next_tool_id("exec");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "execute_query",
            json!({ "sql": sql }),
        );

        let start = std::time::Instant::now();

        let wsc = match self.app_state.ensure_workspace_conn(&self.ws.path).await {
            Ok(w) => w,
            Err(msg) => {
                let full = format!("DuckDB 引擎未就绪: {msg}");
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    full.clone(), Some(sql.to_string()), None, Some(0), None,
                );
                return Err(ToolError(full));
            }
        };
        let conn = wsc.conn.clone();
        let ih = wsc.interrupt_handle.lock().ok().map(|g| g.clone());

        let sql_string = sql.to_string();
        let hard_secs = QUERY_HARD_TIMEOUT_SECS;

        // 执行前检查：解析 SQL 里的表名，查 table_registry access_mode。
        // pushdown 模式的表不能通过视图查询（会拉全表），必须用 postgres_query 下推。
        let ws_path = self.ws.path.clone();
        let sql_for_check = sql_string.clone();
        let pushdown_violation = {
            let ws = ws_path.clone();
            tokio::task::spawn_blocking(move || -> Option<(String, String, String, String)> {
                // 提取 SQL 里的所有 FROM 表名
                let tables = extract_table_names(&sql_for_check);
                for t in &tables {
                    if let Ok(Some(entry)) = crate::db::get_table_registry_by_local_name(&ws, t) {
                        if entry.access_mode == "pushdown" {
                            return Some((t.clone(), entry.connection_name, entry.remote_schema, entry.remote_table));
                        }
                    }
                }
                None
            }).await
        };

        // pushdown 检查
        if let Ok(Some((table_name, conn_name, schema, remote_table))) = &pushdown_violation {
            let msg = format!(
                "表 `{}` 是 pushdown 模式（Hologres 外部库外表），不能通过视图查询（会拉全表到本地，非常慢）。\n\
                 请用 postgres_query 下推查询：\n\
                 SELECT * FROM postgres_query('db_{}', 'SELECT ... FROM \"{}\".\"{}\" WHERE ... GROUP BY ...')",
                table_name, conn_name, schema, remote_table
            );
            emit_tool_result(
                &self.window, &self.task_id, &call_id, "error",
                msg.clone(), Some(sql_string.clone()), None, Some(0), None,
            );
            return Err(ToolError(msg));
        }

        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<SqlResult, String> {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(50)).map_err(|e| {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                if lower.contains("permission denied") || lower.contains("access denied") {
                    format!("查询失败：当前用户没有查询权限。错误: {msg}\n建议：检查数据源连接的用户是否有该表的查询权限，或换一张表。")
                } else if lower.contains("does not exist") || lower.contains("not found") {
                    format!("查询失败：表或视图不存在。错误: {msg}\n建议：用 list_tables 确认表名是否正确，或用 list_remote_tables 重新探查。")
                } else {
                    format!("SQL 执行出错: {msg}")
                }
            })
        });
        let query_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(ToolError)),
                Err(_) => {
                    if let Some(ih) = ih {
                        ih.interrupt();
                    }
                    Err(ToolError(format!("SQL 执行已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(ToolError))
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match query_res {
            Ok(res) => {
                let n = res.rows.len();
                let summary = format!("查询成功，返回 {} 行（{} 列）", n, res.columns.len());
                // 给 LLM 的紧凑文本（避免 50 行灌满上下文）；
                // 完整结构化 SqlResult 通过 payload 发给前端。
                let mut out = String::new();
                out.push_str(&format!("查询成功，返回 {} 行。列: {}\n", n, res.columns.join(", ")));
                for (i, row) in res.rows.iter().enumerate() {
                    let row_str: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                    out.push_str(&format!("行 #{}: {}\n", i + 1, row_str.join(" | ")));
                }
                if res.truncated {
                    out.push_str("(结果已截断，仅返回前 50 行)\n");
                }
                let payload = serde_json::to_value(&res).ok();
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, Some(sql.to_string()), payload, Some(elapsed), None,
                );
                Ok(out)
            }
            Err(err) => {
                // 查询失败时更新 OKF 表探索状态（如果 SQL 引用了已注册的表）。
                let ws_dir = self.ws.dir.to_string_lossy().to_string();
                let err_msg = err.0.clone();
                let sql_clone = sql.to_string();
                let _ = tokio::task::spawn_blocking(move || {
                    // 从 SQL 里提取表名（FROM 后面的词）。
                    if let Some(table_name) = extract_table_from_sql(&sql_clone) {
                        let (status, reason) = classify_query_error(&err_msg);
                        crate::okf::update_table_status(&ws_dir, &table_name, &status, Some(&reason));
                    }
                }).await;
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), Some(sql.to_string()), None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}

/// 从 SQL 中提取 FROM 后面的表名（简单解析，取第一个表名）。
fn extract_table_from_sql(sql: &str) -> Option<String> {
    let upper = sql.to_uppercase();
    if let Some(pos) = upper.find("FROM") {
        let rest = sql[pos + 4..].trim();
        // 跳过前导括号/空格，取第一个 token。
        let token = rest.trim_start_matches('(').trim();
        let end = token.find(|c: char| c.is_whitespace() || c == ',' || c == ')').unwrap_or(token.len());
        let name = token[..end].trim().trim_matches('"').to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// 根据查询错误判定可用性等级。
fn classify_query_error(err: &str) -> (String, String) {
    let lower = err.to_lowercase();
    if lower.contains("storage tier") || lower.contains("lower meta") || lower.contains("odps") {
        ("unavailable_permanent".to_string(), "MaxCompute 非标准存储，Hologres 不支持访问".to_string())
    } else if lower.contains("unsupported type") {
        ("unavailable_permanent".to_string(), "列类型不兼容".to_string())
    } else if lower.contains("permission denied") || lower.contains("access denied") {
        ("unavailable_temporary".to_string(), "权限不足".to_string())
    } else if lower.contains("timeout") || lower.contains("connection") {
        ("unavailable_temporary".to_string(), "连接超时".to_string())
    } else {
        // 未知错误不改状态（可能是 SQL 语法错误，不是表不可用）。
        ("available".to_string(), String::new())
    }
}

/// 从 SQL 里提取所有 FROM 后面的表名（简单解析）。
/// 匹配 `FROM v_xxx`、`FROM "v_xxx"`、`JOIN v_xxx` 等。
fn extract_table_names(sql: &str) -> Vec<String> {
    let mut result = Vec::new();
    let upper = sql.to_uppercase();
    let mut search_from = 0;
    loop {
        // 找 FROM 或 JOIN 关键词
        let from_pos = upper[search_from..].find(" FROM");
        let join_pos = upper[search_from..].find(" JOIN");
        let pos = match (from_pos, join_pos) {
            (Some(f), Some(j)) => search_from + f.min(j),
            (Some(f), None) => search_from + f,
            (None, Some(j)) => search_from + j,
            (None, None) => break,
        };
        // 跳过关键词
        let rest = &sql[pos..];
        let keyword_len = if upper[pos..].starts_with(" FROM") { 5 } else { 5 }; // " FROM" / " JOIN"
        let after = &rest[keyword_len..];
        // 跳过前导空格和括号
        let trimmed = after.trim_start_matches(|c: char| c.is_whitespace() || c == '(');
        // 取第一个 token（表名）
        let mut end = 0;
        let chars = trimmed.char_indices();
        let mut in_quotes = false;
        for (i, c) in chars {
            if c == '"' { in_quotes = !in_quotes; continue; }
            if !in_quotes && (c.is_whitespace() || c == ',' || c == ')' || c == ';') {
                end = i;
                break;
            }
            end = i + c.len_utf8();
        }
        if end > 0 {
            let name = trimmed[..end].trim_matches('"').to_string();
            if !name.is_empty() && !name.to_lowercase().starts_with('(') {
                result.push(name);
            }
        }
        search_from = pos + keyword_len;
    }
    result
}
