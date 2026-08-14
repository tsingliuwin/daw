use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct SearchOkfKnowledgeArgs {
    query: String,
}

pub struct SearchOkfKnowledgeTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for SearchOkfKnowledgeTool {
    const NAME: &'static str = "search_okf_knowledge";
    type Error = ToolError;
    type Args = SearchOkfKnowledgeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "search_okf_knowledge".to_string(),
            description: "在本地 OKF 知识库搜索已沉淀的知识（全局业务概念/公司背景 + 工作区表的字段释义/关联/排障记录）。当需要复用已有业务背景、字段含义、排障经验而非重新探索时调用。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索关键词" }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "search_okf_knowledge", json!({
            "query": &args.query,
        }));

        let start = std::time::Instant::now();
        let ws_path = self.ws.path.clone();
        let query = args.query.clone();
        let hits = tokio::task::spawn_blocking(move || {
            crate::okf::Okf::production().search(&ws_path, &query)
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        let summary = format!("找到 {} 条匹配的知识", hits.len());
        emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), None, None, Some(elapsed), None);
        if hits.is_empty() {
            Ok("未找到匹配的知识。".to_string())
        } else {
            let formatted: Vec<String> = hits
                .iter()
                .map(|h| format!("📄 {}\n{}", h.rel_path, h.preview))
                .collect();
            Ok(formatted.join("\n\n---\n\n"))
        }
    }
}
