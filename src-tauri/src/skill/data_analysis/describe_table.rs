use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::{execute, QUERY_HARD_TIMEOUT_SECS};
use super::super::super::model::SqlResult;
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct DescribeTableArgs {
    table_name: String,
}

pub struct DescribeTableTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for DescribeTableTool {
    const NAME: &'static str = "describe_table";
    type Error = ToolError;
    type Args = DescribeTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "describe_table".to_string(),
            description: "获取指定数据表或视图的结构信息（列名、数据类型等）。在对表编写 SQL 前，必须调用此工具了解其结构。table_name 可用全限定名（db_xxx.orders）或短名（orders）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "要查询结构的表名或视图名" }
                },
                "required": ["table_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let table_name = args.table_name.trim();
        // 白名单校验：允许字母数字下划线、点（catalog.table）、双引号（标识符转义）。
        if !table_name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '"') {
            return Err(ToolError("表名包含非法字符，仅允许字母、数字、下划线、点和引号。".to_string()));
        }

        let call_id = next_tool_id("desc");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "describe_table",
            json!({ "table_name": table_name }),
        );

        let start = std::time::Instant::now();
        let duckdb_guard = self.app_state.duckdb.lock().await;
        let conn = match &*duckdb_guard {
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
        let ih = self.app_state.interrupt_handle.lock().await.as_ref()
            .and_then(|h| h.lock().ok().map(|g| g.clone()));

        let table_name_string = table_name.to_string();
        let hard_secs = QUERY_HARD_TIMEOUT_SECS;
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<(SqlResult, Option<String>, std::collections::HashMap<String, String>, Vec<String>), String> {
            let guard = conn.blocking_lock();
            let sql = format!("DESCRIBE {}", table_name_string);
            let query_res = execute::run_query(&guard, &sql, None).map_err(|e| e.to_string())?;

            // OKF：解析表的业务释义和关联关系。
            // 如果 OKF 文件不存在，用 DESCRIBE 结果自动生成骨架（空业务释义）。
            let okf_file = crate::okf::get_okf_dir(&ws_dir)
                .join("tables")
                .join(format!("{table_name_string}.md"));
            if !okf_file.exists() {
                // 自动生成骨架：从 DESCRIBE 结果提取列信息。
                let columns: Vec<crate::okf::ColumnInfo> = query_res.rows.iter().map(|r| {
                    let name = r.get(0).map(|v: &serde_json::Value| v.to_string().trim_matches('"').to_string()).unwrap_or_default();
                    let ty = r.get(1).map(|v: &serde_json::Value| v.to_string()).unwrap_or_default();
                    let nullable = r.get(2).map(|v: &serde_json::Value| v.to_string().to_uppercase().contains("YES")).unwrap_or(true);
                    (name, ty, nullable)
                }).collect();
                let _ = crate::okf::write_table_okf(&ws_dir, &table_name_string, &columns, None);
            }
            let (okf_title, col_comments, relations) = crate::okf::parse_column_semantics(&ws_dir, &table_name_string);
            Ok((query_res, okf_title, col_comments, relations))
        });
        let desc_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(|e| ToolError(format!("执行 DESCRIBE 失败: {e}")))),
                Err(_) => {
                    if let Some(ih) = ih {
                        ih.interrupt();
                    }
                    Err(ToolError(format!("查询已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("执行 DESCRIBE 失败: {e}"))))
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match desc_res {
            Ok((res, okf_title, col_comments, relations)) => {
                let n = res.rows.len();
                let col_lines: Vec<String> = res.rows.iter().map(|r| {
                    let name = r.get(0).map(|v: &serde_json::Value| v.to_string()).unwrap_or_default();
                    let ty = r.get(1).map(|v: &serde_json::Value| v.to_string()).unwrap_or_default();
                    let null = r.get(2).map(|v: &serde_json::Value| v.to_string()).unwrap_or_default();
                    let clean_name = name.trim_matches('"').to_string();
                    let comment = col_comments.get(&clean_name).map(|c| format!(", 释义: {c}")).unwrap_or_default();
                    format!("{clean_name} (类型: {ty}, 允许空: {null}){comment}")
                }).collect();

                let mut title_part = String::new();
                if let Some(t) = &okf_title {
                    title_part = format!(" (业务名称: {t})");
                }
                let mut rels_part = String::new();
                if !relations.is_empty() {
                    rels_part = format!("\n\n关联关系:\n{}", relations.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n"));
                }

                let summary = format!("结构分析完成，{}{} 共 {} 个字段", table_name, title_part, n);
                let payload = serde_json::to_value(&res).ok();
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, payload, Some(elapsed), None,
                );
                Ok(format!("表 {}{} 的列结构如下:\n{}{}", table_name, title_part, col_lines.join("\n"), rels_part))
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
