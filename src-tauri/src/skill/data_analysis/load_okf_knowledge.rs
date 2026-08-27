use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct LoadOkfKnowledgeArgs {
    category: String,
    name: String,
    heading: String,
}

pub struct LoadOkfKnowledgeTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for LoadOkfKnowledgeTool {
    const NAME: &'static str = "load_okf_knowledge";
    type Error = ToolError;
    type Args = LoadOkfKnowledgeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "load_okf_knowledge".to_string(),
            description: "读取本地 OKF 知识库某条知识的细节。可读某文件下指定标题（精简读取以省 token），或用 heading=all 读整篇全文。运行时上下文已注入大纲，按需精读具体条目即可。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": format!("OKF 类别：{}", crate::okf::model::Category::prompt_list()) },
                    "name": { "type": "string", "description": "文件名（不含 .md 后缀），如 concepts 的 company_profile、表的 v_orders" },
                    "heading": { "type": "string", "description": "要读取的标题，如：业务描述、关联关系、异常排障记录。填 all（或留空）返回整个文件全文（文件存在即成功）" }
                },
                "required": ["category", "name", "heading"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "load_okf_knowledge", json!({
            "category": &args.category, "name": &args.name, "heading": &args.heading,
        }));

        let start = std::time::Instant::now();
        let cat = crate::okf::model::Category::from_str(&args.category)
            .ok_or_else(|| ToolError(format!("未知知识类别: {}", args.category)))?;
        let ws_path = self.ws.path.clone();
        let name = args.name.clone();
        let heading = args.heading.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::okf::Okf::production().read(&ws_path, cat, &name, &heading)
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(o) => {
                let scope_cn = o.scope.label();
                let heading_label = if args.heading.eq_ignore_ascii_case("all") || args.heading.trim().is_empty() {
                    "全文"
                } else {
                    args.heading.as_str()
                };
                let summary = format!("读取【{}】{}/{} 的「{}」", scope_cn, args.category, args.name, heading_label);
                let detail = Some(o.content.clone());
                let payload = serde_json::json!({
                    "scope": scope_cn,
                    "file": o.file_path.to_string_lossy(),
                });
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, detail, Some(payload), Some(elapsed), None,
                );
                Ok(o.content)
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.clone(), None, None, Some(elapsed), None);
                Err(ToolError(err))
            }
        }
    }
}
