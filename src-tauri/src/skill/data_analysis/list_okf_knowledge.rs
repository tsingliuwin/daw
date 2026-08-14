use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ListOkfKnowledgeArgs {}

pub struct ListOkfKnowledgeTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for ListOkfKnowledgeTool {
    const NAME: &'static str = "list_okf_knowledge";
    type Error = ToolError;
    type Args = ListOkfKnowledgeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_okf_knowledge".to_string(),
            description: "查看知识库大纲：列出全局业务概念 + 当前工作区已注册的表(含探索状态/字段释义)/视图/数据源/排障记录。用户问'有哪些知识/表/概念'、或想确认已沉淀内容、或开场后新增了知识想刷新大纲时调用。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "list_okf_knowledge", json!({}));

        let start = std::time::Instant::now();
        let ws_path = self.ws.path.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let entries = crate::db::list_table_registry(&ws_path).unwrap_or_default();
            Ok(crate::okf::Okf::production().catalog_summary(&ws_path, &entries))
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(outline) => {
                let empty = outline.trim().is_empty();
                let summary = if empty {
                    "知识库为空（尚无沉淀）".to_string()
                } else {
                    "知识库大纲（全局概念 + 工作区表/视图/数据源/排障）".to_string()
                };
                let detail = if empty { None } else { Some(outline.clone()) };
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), detail, None, Some(elapsed), None,
                );
                Ok(if empty {
                    "知识库暂无沉淀。".to_string()
                } else {
                    outline
                })
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.clone(), None, None, Some(elapsed), None);
                Err(ToolError(err))
            }
        }
    }
}
