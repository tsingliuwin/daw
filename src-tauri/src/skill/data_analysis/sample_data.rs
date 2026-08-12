use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::{execute, QUERY_HARD_TIMEOUT_SECS};
use super::super::super::model::SqlResult;
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
                    "table_name": { "type": "string", "description": "要采样查询的表名或视图名" }
                },
                "required": ["table_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let table_name = args.table_name.trim();
        if table_name.contains('.') && !table_name.starts_with('"') {
            return Err(ToolError(
                format!("sample_data 只能对注册后的短名使用（如 v_orders），不能直接用远程表名「{table_name}」。请先调用 register_table 注册，再用短名调用。")
            ));
        }

        let call_id = next_tool_id("sample");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "sample_data",
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
        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<SqlResult, String> {
            let guard = conn.blocking_lock();
            let sql = format!("SELECT * FROM {} LIMIT 5", table_name_string);
            execute::run_query(&guard, &sql, Some(5))
        });
        let query_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(|e| ToolError(format!("采样查询失败: {e}")))),
                Err(_) => {
                    if let Some(ih) = ih {
                        ih.interrupt();
                    }
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
                let summary = format!("完成采样，获取到 {} 行样例数据", n);
                let payload = serde_json::to_value(&res).ok();
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, payload, Some(elapsed), None,
                );
                Ok(format!("表 {} 的前 {} 行样例数据已展示在结果表格中。", table_name, n))
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
