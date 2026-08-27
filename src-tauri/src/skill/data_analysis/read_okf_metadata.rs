use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ReadOkfMetadataArgs {
    category: String,
    name: String,
}

pub struct ReadOkfMetadataTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for ReadOkfMetadataTool {
    const NAME: &'static str = "read_okf_metadata";
    type Error = ToolError;
    type Args = ReadOkfMetadataArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "read_okf_metadata".to_string(),
            description: "读取某条知识的元数据（frontmatter：type/title/description/created_at/updated_at），不含正文。想看创建/更新时间或描述时用。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": format!("OKF 类别：{}", crate::okf::model::Category::prompt_list()) },
                    "name": { "type": "string", "description": "文件名（不含 .md 后缀）" }
                },
                "required": ["category", "name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "read_okf_metadata", json!({
            "category": &args.category, "name": &args.name,
        }));

        let start = std::time::Instant::now();
        let cat = crate::okf::model::Category::from_str(&args.category)
            .ok_or_else(|| ToolError(format!("未知知识类别: {}", args.category)))?;
        let scope_cn = cat.scope().label();
        let ws_path = self.ws.path.clone();
        let name = args.name.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::okf::Okf::production().read_metadata(&ws_path, cat, &name)
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(fm) => {
                let summary = format!(
                    "读取【{}】{}/{} 的元数据（{} 个字段）",
                    scope_cn, args.category, args.name, fm.entries.len()
                );
                let mut text = String::new();
                for (k, v) in &fm.entries {
                    text.push_str(&format!("{k}: {v}\n"));
                }
                let detail = Some(text.clone());
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), detail, None, Some(elapsed), None,
                );
                Ok(text.trim().to_string())
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.clone(), None, None, Some(elapsed), None);
                Err(ToolError(err))
            }
        }
    }
}
