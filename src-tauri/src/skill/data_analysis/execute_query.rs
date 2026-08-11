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

        let conn = match &self.app_state.duckdb {
            Some(c) => c.clone(),
            None => {
                let msg = "DuckDB 引擎未初始化，无法执行查询。请检查应用日志。".to_string();
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    msg.clone(), Some(sql.to_string()), None, Some(0), None,
                );
                return Err(ToolError(msg));
            }
        };
        let ih = self.app_state.interrupt_handle.as_ref()
            .and_then(|h| h.lock().ok().map(|g| g.clone()));

        let sql_string = sql.to_string();
        let hard_secs = QUERY_HARD_TIMEOUT_SECS;
        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<SqlResult, String> {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(50)).map_err(|e| format!("SQL 执行出错: {e}"))
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
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), Some(sql.to_string()), None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}
