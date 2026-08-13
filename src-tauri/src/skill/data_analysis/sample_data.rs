use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::execute;
use super::super::super::duckdb::attach::workspace_attach_alias;
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct SampleDataArgs {
    table_name: String,
}

pub struct SampleDataTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for SampleDataTool {
    const NAME: &'static str = "sample_data";
    type Error = ToolError;
    type Args = SampleDataArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "sample_data".to_string(),
            description: "获取已注册表/视图的前 5 行样例数据。只能对 register_table 注册后的短名（如 v_orders）使用。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "注册后的短名（如 v_orders）" }
                },
                "required": ["table_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let table_name = args.table_name.trim().to_string();
        if table_name.contains('.') && !table_name.starts_with('"') {
            return Err(ToolError(
                format!("sample_data 只能对注册后的短名使用（如 v_orders），不能直接用远程表名「{table_name}」。请先调用 register_table 注册，再用短名调用。")
            ));
        }

        let call_id = next_tool_id("sample");
        emit_tool_call(&self.window, &self.task_id, &call_id, "sample_data", json!({
            "table_name": &table_name,
        }));

        let start = std::time::Instant::now();

        // 查 table_registry 获取 access_mode
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let ws_path_clone = ws_path.clone();
        let table_name_clone = table_name.clone();
        let registry_entry = {
            let tn = table_name.clone();
            let ws = ws_path.clone();
            tokio::task::spawn_blocking(move || {
                crate::db::get_table_registry_by_local_name(&ws, &tn)
            }).await
            .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
            .map_err(|e| ToolError(e))?
        };

        let duckdb_guard = self.app_state.duckdb.lock().await;
        let conn = match &*duckdb_guard {
            Some(c) => c.clone(),
            None => {
                let msg = "DuckDB 引擎未初始化。".to_string();
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), None, None, Some(0), None);
                return Err(ToolError(msg));
            }
        };
        let ih = self.app_state.interrupt_handle.lock().await.as_ref()
            .and_then(|h| h.lock().ok().map(|g| g.clone()));

        let hard_secs = super::super::super::duckdb::QUERY_HARD_TIMEOUT_SECS;

        // 在闭包前构造实际 SQL，便于在结果中透出——调用方能从 detail 看出
        // 走的是 catalog（SELECT * FROM "v_xxx"）还是 pushdown（postgres_query 下推）。
        let access_mode = registry_entry
            .as_ref()
            .map(|e| e.access_mode.as_str())
            .unwrap_or("catalog")
            .to_string();
        let sql = match &registry_entry {
            Some(entry) if entry.access_mode == "pushdown" => {
                let catalog = workspace_attach_alias(&entry.connection_name);
                format!("SELECT * FROM postgres_query('{}', 'SELECT * FROM \"{}\".\"{}\" LIMIT 5')",
                    catalog, entry.remote_schema, entry.remote_table)
            }
            _ => format!("SELECT * FROM \"{}\" LIMIT 5", table_name.replace('"', "\"\"")),
        };
        let sql_for_query = sql.clone();

        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<crate::model::SqlResult, String> {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_for_query, Some(5))
        });

        let query_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(|e| ToolError(format!("采样查询失败: {e}")))),
                Err(_) => {
                    if let Some(ih) = ih { ih.interrupt(); }
                    Err(ToolError(format!("采样查询已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("采样查询失败: {e}"))))
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match query_res {
            Ok(res) => {
                let n = res.rows.len();
                let summary = format!("完成采样（access_mode={}），获取到 {} 行样例数据", access_mode, n);
                let detail = sql.clone();
                let payload = serde_json::to_value(&res).ok();
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, Some(detail), payload, Some(elapsed), None);
                Ok(format!("表 {} 的前 {} 行样例数据已展示在结果表格中。", table_name, n))
            }
            Err(err) => {
                let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
                let err_msg = err.0.clone();
                let table_name_clone2 = table_name.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let (status, reason) = classify_sample_error(&err_msg);
                    crate::db::update_table_registry_status(&ws_dir, &table_name_clone2, &status, Some(&reason));
                }).await;
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), None, None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}

fn classify_sample_error(err: &str) -> (String, String) {
    let lower = err.to_lowercase();
    if lower.contains("storage tier") || lower.contains("lower meta") {
        ("unavailable_permanent".to_string(), "MaxCompute 非标准存储，Hologres 不支持访问".to_string())
    } else if lower.contains("unsupported type") {
        ("unavailable_permanent".to_string(), "列类型不兼容".to_string())
    } else if lower.contains("permission") || lower.contains("privilege") || lower.contains("authorize") || lower.contains("deny") {
        ("unavailable_temporary".to_string(), "权限不足".to_string())
    } else if lower.contains("timeout") || lower.contains("connection") {
        ("unavailable_temporary".to_string(), "连接超时".to_string())
    } else {
        ("available".to_string(), String::new())
    }
}
