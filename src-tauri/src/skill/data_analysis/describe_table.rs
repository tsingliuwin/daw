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
            description: "获取已注册表/视图的结构信息（列名、数据类型、业务释义）。只能对 register_table 注册后的短名（如 v_orders）使用，不能直接用远程表名（如 default.orders）。".to_string(),
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
        // 拒绝远程表名（schema.table 格式），避免触发 postgres 元数据扫描。
        // agent 必须先 register_table 注册短名，再用短名调 describe_table。
        if table_name.contains('.') && !table_name.starts_with('"') {
            return Err(ToolError(
                format!("describe_table 只能对注册后的短名使用（如 v_orders），不能直接用远程表名「{table_name}」。请先调用 register_table 注册，再用短名调用。")
            ));
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
            // 对 schema.table 格式的表名，每段加双引号转义保留字（如 default）。
            let quoted = table_name_string.split('.')
                .map(|p| format!("\"{}\"", p.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(".");
            let sql = format!("DESCRIBE {}", quoted);
            let query_res = execute::run_query(&guard, &sql, None).map_err(|e| e.to_string())?;

            // OKF：解析表的业务释义和关联关系。
            // 如果 OKF 文件不存在，用 DESCRIBE 结果自动生成骨架（空业务释义）。
            // OKF 文件名取最后一段短名（v_orders 而非 db_xxx.v_orders），
            // 和 agent 调 write_okf_block 时用的短名一致。
            let okf_short_name = table_name_string.rsplit('.').next().unwrap_or(&table_name_string).to_string();
            let okf_file = crate::okf::get_okf_dir(&ws_dir)
                .join("tables")
                .join(format!("{okf_short_name}.md"));
            if !okf_file.exists() {
                // 自动生成骨架：从 DESCRIBE 结果提取列信息。
                let columns: Vec<crate::okf::ColumnInfo> = query_res.rows.iter().map(|r| {
                    let name = r.get(0).map(|v: &serde_json::Value| v.to_string().trim_matches('"').to_string()).unwrap_or_default();
                    let ty = r.get(1).map(|v: &serde_json::Value| v.to_string()).unwrap_or_default();
                    let nullable = r.get(2).map(|v: &serde_json::Value| v.to_string().to_uppercase().contains("YES")).unwrap_or(true);
                    (name, ty, nullable)
                }).collect();
                let _ = crate::okf::write_table_okf(&ws_dir, &okf_short_name, &columns, None);
            }
            let (okf_title, col_comments, relations) = crate::okf::parse_column_semantics(&ws_dir, &okf_short_name);
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
