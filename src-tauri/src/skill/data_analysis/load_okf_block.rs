use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct LoadOkfBlockArgs {
    category: String,
    name: String,
    heading: String,
}

pub struct LoadOkfBlockTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for LoadOkfBlockTool {
    const NAME: &'static str = "load_okf_block";
    type Error = ToolError;
    type Args = LoadOkfBlockArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "load_okf_block".to_string(),
            description: "读取本地 OKF 知识库中某个文件下指定二级标题的内容。用于精简读取业务释义、关联关系、排障记录等，避免加载全文节省 token。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "OKF 目录类别：tables, views, concepts, pipelines/specific" },
                    "name": { "type": "string", "description": "文件名（不含 .md 后缀），如表名 orders" },
                    "heading": { "type": "string", "description": "要读取的标题，如：关联关系、异常排障记录、业务描述" }
                },
                "required": ["category", "name", "heading"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "load_okf_block", json!({
            "category": &args.category, "name": &args.name, "heading": &args.heading,
        }));

        let start = std::time::Instant::now();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let category = args.category.clone();
        let name = args.name.clone();
        let heading = args.heading.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::okf::read_okf_block(&ws_dir, &category, &name, &heading)
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(content) => {
                let summary = format!("读取 OKF: {}/{}/{}", args.category, args.name, args.heading);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, None, None, Some(elapsed), None);
                Ok(content)
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.clone(), None, None, Some(elapsed), None);
                Err(ToolError(err))
            }
        }
    }
}
