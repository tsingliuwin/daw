use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct WriteOkfBlockArgs {
    category: String,
    name: String,
    heading: String,
    content: String,
}

pub struct WriteOkfBlockTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for WriteOkfBlockTool {
    const NAME: &'static str = "write_okf_block";
    type Error = ToolError;
    type Args = WriteOkfBlockArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "write_okf_block".to_string(),
            description: "向本地 OKF 知识库写入或更新业务知识。当用户补充了字段释义、关联关系、排障经验时必须立即调用。类别选择：concepts=全局业务概念/公司背景；tables/views=单表字段释义/关联；pipelines/specific=清洗配方/排障。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "OKF 目录类别：tables, views, concepts, pipelines/specific" },
                    "name": { "type": "string", "description": "文件名（不含 .md 后缀）" },
                    "heading": { "type": "string", "description": "要写入的标题，如：关联关系、异常排障记录、业务描述" },
                    "content": { "type": "string", "description": "要写入的内容（Markdown 格式）" }
                },
                "required": ["category", "name", "heading", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "write_okf_block", json!({
            "category": &args.category, "name": &args.name, "heading": &args.heading,
        }));

        let start = std::time::Instant::now();
        let ws_dir = self.ws.dir.to_string_lossy().to_string();
        let category = args.category.clone();
        let name = args.name.clone();
        let heading = args.heading.clone();
        let content = args.content.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::okf::write_okf_block(&ws_dir, &category, &name, &heading, &content)
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(()) => {
                let summary = format!("已更新 OKF: {}/{}/{}", args.category, args.name, args.heading);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), None, None, Some(elapsed), None);
                Ok(summary)
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.clone(), None, None, Some(elapsed), None);
                Err(ToolError(err))
            }
        }
    }
}
