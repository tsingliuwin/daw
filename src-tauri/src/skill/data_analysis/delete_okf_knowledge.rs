use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct DeleteOkfKnowledgeArgs {
    category: String,
    name: String,
    #[serde(default)]
    merge_into: Option<String>,
}

pub struct DeleteOkfKnowledgeTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for DeleteOkfKnowledgeTool {
    const NAME: &'static str = "delete_okf_knowledge";
    type Error = ToolError;
    type Args = DeleteOkfKnowledgeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "delete_okf_knowledge".to_string(),
            description: "删除一条知识文件（任意类别，含全局 concepts）。主要用于合并去重：先把整合后的完整内容写入保留文件，再调用本工具删除冗余文件，并传 merge_into=保留文件名——全库 [[被删名]] 内链会自动改写指向保留文件。删除会进 git 历史（可恢复），但调用前必须已向用户说明合并/删除方案。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": format!("OKF 类别：{}", crate::okf::model::Category::prompt_list()) },
                    "name": { "type": "string", "description": "要删除的文件名（不含 .md 后缀）" },
                    "merge_into": { "type": "string", "description": "（可选）合并去重时保留的文件名：删除前把全库 [[本文件]] 内链改写为 [[保留文件]]" }
                },
                "required": ["category", "name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "delete_okf_knowledge", json!({
            "category": &args.category, "name": &args.name, "merge_into": &args.merge_into,
        }));

        let start = std::time::Instant::now();
        let cat = crate::okf::model::Category::from_str(&args.category)
            .ok_or_else(|| ToolError(format!("未知知识类别: {}", args.category)))?;
        let scope_cn = cat.scope().label();
        let ws_path = self.ws.path.clone();
        let name = args.name.clone();
        let merge_into = args.merge_into.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::okf::Okf::production().delete_knowledge(&ws_path, cat, &name, merge_into.as_deref())
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(true) => {
                let merge_note = match &args.merge_into {
                    Some(t) => format!("，全库内链已改写指向 {t}"),
                    None => String::new(),
                };
                let summary = format!("已删除【{scope_cn}】{}/{}{merge_note}", args.category, args.name);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), None, None, Some(elapsed), None,
                );
                Ok(summary)
            }
            Ok(false) => {
                let summary = format!("文件不存在，未删除：【{scope_cn}】{}/{}", args.category, args.name);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), None, None, Some(elapsed), None,
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
