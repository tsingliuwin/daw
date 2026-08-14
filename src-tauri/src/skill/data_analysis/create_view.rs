use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};
use tokio::sync::oneshot;

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_awaiting, emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct CreateViewArgs {
    name: String,
    select_sql: String,
}

pub struct CreateViewTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub confirm_mode: String,
    pub ws: crate::skill::WorkspaceRef,
}

/// 名称校验：拒空、拒双引号（防注入）、拒 null byte。
fn sanitize_ddl_ident(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains('"') || trimmed.contains('\0') {
        return Err(format!("非法对象名: {name:?}"));
    }
    Ok(trimmed.to_string())
}

impl Tool for CreateViewTool {
    const NAME: &'static str = "create_view";
    type Error = ToolError;
    type Args = CreateViewArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "create_view".to_string(),
            description: "创建或重建逻辑视图（零拷贝，不拉取数据到本地）。用于封装联邦查询逻辑（如 JOIN 多个数据源的表），持久化在本地，重启后仍可用。命名建议 v_ 前缀（最终视图）或 tmp_v_ 前缀（中间视图）。调用前必须先用 execute_query 验证 select_sql 能跑通。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "视图名（如 v_user_orders）" },
                    "select_sql": { "type": "string", "description": "视图的 SELECT 语句（不带尾部分号）" }
                },
                "required": ["name", "select_sql"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let name = sanitize_ddl_ident(&args.name).map_err(ToolError)?;
        let select_sql = args.select_sql.trim().trim_end_matches(';').to_string();
        // 构建 DDL：先 DROP VIEW + DROP TABLE（防同名类型冲突），再 CREATE VIEW。
        let ddl = format!(
            "DROP VIEW IF EXISTS \"{name}\";\nDROP TABLE IF EXISTS \"{name}\";\nCREATE VIEW \"{name}\" AS {select_sql};"
        );

        let call_id = next_tool_id("cv");
        emit_tool_call(&self.window, &self.task_id, &call_id, "create_view", json!({
            "name": &name, "select_sql": &select_sql,
        }));

        let start = std::time::Instant::now();
        let summary_pending = format!("创建视图 {}", name);

        // 确认 gate：仅"变更前确认"模式才挂起等待用户确认，"自动执行"直接执行。
        let approved = if self.confirm_mode != "变更前确认" {
            true
        } else {
            // 用 pending_confirmations + oneshot 实现
            let (tx, rx) = oneshot::channel::<crate::state::ConfirmDecision>();
            {
                let key = format!("{}:{}", self.task_id, call_id);
                let mut pending = self.app_state.pending_confirmations.lock().await;
                pending.insert(key, crate::state::PendingConfirmation { tx });
            }
            emit_tool_awaiting(&self.window, &self.task_id, &call_id, summary_pending.clone(), ddl.clone());
            match rx.await {
                Ok(d) => d.approved,
                Err(_) => false,
            }
        };

        if !approved {
            let msg = "用户已取消此操作".to_string();
            emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), Some(ddl), None, Some(start.elapsed().as_millis() as u64), None);
            return Err(ToolError(msg));
        }

        // 执行 DDL。
        let wsc = match self.app_state.ensure_workspace_conn(&self.ws.path).await {
            Ok(w) => w,
            Err(msg) => {
                let full = format!("DuckDB 引擎未就绪: {msg}");
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", full.clone(), None, None, Some(0), None);
                return Err(ToolError(full));
            }
        };
        let conn = wsc.conn.clone();
        let ddl_clone = ddl.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let guard = conn.blocking_lock();
            guard.execute_batch(&ddl_clone).map_err(|e| e.to_string())
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(()) => {
                // 写 OKF 视图骨架（幂等，已存在不覆盖）。
                let ws_dir = self.ws.dir.to_string_lossy().to_string();
                let name_clone = name.clone();
                let sql_clone = select_sql.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::okf::Okf::production().ensure_view_skeleton(&ws_dir, &name_clone, &sql_clone)
                }).await;

                let summary = format!("视图 {} 创建成功", name);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), Some(ddl), None, Some(elapsed), None);
                Ok(summary)
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), Some(ddl), None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}
