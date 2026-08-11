use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::attach::workspace_attach_alias;
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
                    "table_name": { "type": "string", "description": "远程表名，含 schema（如 public.orders）" },
                    "local_name": { "type": "string", "description": "本地视图名（可选，默认 v_{table名}）。注册后用此名查询" }
                },
                "required": ["connection_name", "table_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let conn_name = args.connection_name.trim();
        let table_name = args.table_name.trim();
        // 本地视图名：优先用户指定，否则 v_{table 最后一段}。
        let local = args.local_name.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
        let local_name = local.map(|s| s.to_string()).unwrap_or_else(|| {
            // table_name 可能是 schema.table，取最后一段。
            let short = table_name.rsplit('.').next().unwrap_or(table_name);
            format!("v_{short}")
        });

        // 校验名称合法性。
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
        let catalog = workspace_attach_alias(conn_name);
        let remote_full = format!("{catalog}.{table_name}");

        let conn = match &self.app_state.duckdb {
            Some(c) => c.clone(),
            None => {
                let msg = "DuckDB 引擎未初始化。".to_string();
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), None, None, Some(0), None);
                return Err(ToolError(msg));
            }
        };

        let local_name_clone = local_name.clone();
        let remote_full_clone = remote_full.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let guard = conn.blocking_lock();
            // CREATE OR REPLACE 视图（覆盖同名旧视图）。
            let sql = format!(
                "CREATE OR REPLACE VIEW \"{}\" AS SELECT * FROM {};",
                local_name_clone, remote_full_clone
            );
            guard.execute_batch(&sql).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(()) => {
                let summary = format!("已注册视图 {} → {}", local_name, remote_full);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), None, None, Some(elapsed), None);
                Ok(format!("{summary}。后续可用 {} 作为表名查询。", local_name))
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), None, None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}
