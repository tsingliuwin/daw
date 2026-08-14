use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct RenameOkfKnowledgeArgs {
    category: String,
    old_name: String,
    new_name: String,
}

pub struct RenameOkfKnowledgeTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for RenameOkfKnowledgeTool {
    const NAME: &'static str = "rename_okf_knowledge";
    type Error = ToolError;
    type Args = RenameOkfKnowledgeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "rename_okf_knowledge".to_string(),
            description: "重命名知识文件（任意类别，含全局 concepts）：移动文件、更新 frontmatter title，并把全库 [[旧名]] 内链同步改写为 [[新名]]。用于规范化命名（如中文名改英文、统一前缀），不改变正文内容。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "OKF 类别：concepts/tables/views/sources/pipelines/specific" },
                    "old_name": { "type": "string", "description": "现有文件名（不含 .md 后缀）" },
                    "new_name": { "type": "string", "description": "新文件名（不含 .md 后缀，不能与同类目已有文件重名）" }
                },
                "required": ["category", "old_name", "new_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "rename_okf_knowledge", json!({
            "category": &args.category, "old_name": &args.old_name, "new_name": &args.new_name,
        }));

        let start = std::time::Instant::now();
        let cat = crate::okf::model::Category::from_str(&args.category)
            .ok_or_else(|| ToolError(format!("未知知识类别: {}", args.category)))?;
        let scope_cn = cat.scope().label();
        let ws_path = self.ws.path.clone();
        let old_name = args.old_name.clone();
        let new_name = args.new_name.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::okf::Okf::production().rename_knowledge(&ws_path, cat, &old_name, &new_name)
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(path) => {
                let summary = format!(
                    "已重命名【{scope_cn}】{}/{} → {}（title 与全库内链已同步）",
                    args.category, args.old_name, args.new_name
                );
                let payload = serde_json::json!({
                    "scope": scope_cn,
                    "file": path.to_string_lossy(),
                });
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), None, Some(payload), Some(elapsed), None,
                );
                Ok(summary)
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.clone(), None, None, Some(elapsed), None);
                Err(ToolError(err))
            }
        }
    }
}
