use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct UpdateOkfMetadataArgs {
    category: String,
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

pub struct UpdateOkfMetadataTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for UpdateOkfMetadataTool {
    const NAME: &'static str = "update_okf_metadata";
    type Error = ToolError;
    type Args = UpdateOkfMetadataArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "update_okf_metadata".to_string(),
            description: "修改某条知识的元数据（title/description/status），不改正文。精炼标题或补充一句话描述时用；新结论推翻旧知识时，把旧文件 status 置为 superseded（大纲会标注「已作废」，不再当现行权威）。updated_at 自动刷新。至少提供三者之一。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": format!("OKF 类别：{}", crate::okf::model::Category::prompt_list()) },
                    "name": { "type": "string", "description": "文件名（不含 .md 后缀）" },
                    "title": { "type": "string", "description": "（可选）新的标题" },
                    "description": { "type": "string", "description": "（可选）新的一句话描述" },
                    "status": { "type": "string", "enum": ["active", "superseded"], "description": "（可选）生命周期状态：superseded=已被新知取代（大纲标注已作废）；active=恢复为现行" }
                },
                "required": ["category", "name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "update_okf_metadata", json!({
            "category": &args.category, "name": &args.name,
            "title": &args.title, "description": &args.description,
            "status": &args.status,
        }));

        let start = std::time::Instant::now();
        let cat = crate::okf::model::Category::from_str(&args.category)
            .ok_or_else(|| ToolError(format!("未知知识类别: {}", args.category)))?;
        let scope_cn = cat.scope().label();
        let ws_path = self.ws.path.clone();
        let name = args.name.clone();
        let title = args.title.clone();
        let description = args.description.clone();
        let status = args.status.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
            let mut fields: Vec<(String, String)> = Vec::new();
            let mut changed: Vec<String> = Vec::new();
            if let Some(t) = title {
                fields.push(("title".into(), t));
                changed.push("title".into());
            }
            if let Some(d) = description {
                fields.push(("description".into(), d));
                changed.push("description".into());
            }
            if let Some(s) = status {
                if s != "active" && s != "superseded" {
                    return Err("status 只能为 active 或 superseded".to_string());
                }
                changed.push(format!("status={s}"));
                fields.push(("status".into(), s));
            }
            if fields.is_empty() {
                return Err("至少提供 title、description 或 status 之一".to_string());
            }
            crate::okf::Okf::production().update_metadata(&ws_path, cat, &name, &fields)?;
            Ok(changed)
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(changed) => {
                let summary = format!(
                    "已更新【{}】{}/{} 的元数据（{}）",
                    scope_cn, args.category, args.name, changed.join("、")
                );
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
