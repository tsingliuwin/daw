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
        let ws_path_clone = self.app_state.workspace_path.lock().await.clone();
        // 查 table_registry 获取 access_mode
        let registry_entry = {
            let tn = table_name_string.clone();
            let ws = ws_path_clone.clone();
            tokio::task::spawn_blocking(move || crate::db::get_table_registry_by_local_name(&ws, &tn))
                .await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
                .map_err(|e| ToolError(e))?
        };
        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<(SqlResult, Option<String>, std::collections::HashMap<String, String>, Vec<String>), String> {
            let guard = conn.blocking_lock();
            // 根据 access_mode 选择 SQL
            let sql = match &registry_entry {
                Some(entry) if entry.access_mode == "pushdown" => {
                    let catalog = crate::duckdb::attach::workspace_attach_alias(&entry.connection_name);
                    format!("SELECT * FROM postgres_query('{}', 'SELECT * FROM \"{}\".\"{}\" LIMIT 0')",
                        catalog, entry.remote_schema, entry.remote_table)
                }
                _ => format!("SELECT * FROM \"{}\" LIMIT 0", table_name_string.replace('"', "\"\"")),
            };
            let query_res = execute::run_query(&guard, &sql, None).map_err(|e| e.to_string())?;

            // OKF：解析表的业务释义和关联关系。
            let okf_short_name = table_name_string.clone();
            let okf_file = crate::okf::get_okf_dir(&ws_dir)
                .join("tables")
                .join(format!("{okf_short_name}.md"));
            if !okf_file.exists() {
                // 用 columns/column_types 生成骨架（替代 DESCRIBE 的 rows）。
                let columns: Vec<crate::okf::ColumnInfo> = query_res.columns.iter().enumerate().map(|(i, name)| {
                    let ty = query_res.column_types.get(i).cloned().unwrap_or_default();
                    (name.clone(), ty, true)
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
                let n = res.columns.len();
                let col_lines: Vec<String> = res.columns.iter().enumerate().map(|(i, name)| {
                    let ty = res.column_types.get(i).cloned().unwrap_or_default();
                    let comment = col_comments.get(name).map(|c| format!(", 释义: {c}")).unwrap_or_default();
                    format!("{name} (类型: {ty}){comment}")
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
