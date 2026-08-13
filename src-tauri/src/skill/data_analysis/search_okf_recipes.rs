use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct SearchOkfRecipesArgs {
    query: String,
}

pub struct SearchOkfRecipesTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for SearchOkfRecipesTool {
    const NAME: &'static str = "search_okf_recipes";
    type Error = ToolError;
    type Args = SearchOkfRecipesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "search_okf_recipes".to_string(),
            description: "在本地 OKF 知识库的 pipelines 目录下搜索导入/清洗配方或排障记录。当遇到数据清洗困难（如解析特殊日期、编码报错）时调用。".to_string(),
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
        emit_tool_call(&self.window, &self.task_id, &call_id, "search_okf_recipes", json!({
            "query": &args.query,
        }));

        let start = std::time::Instant::now();
        let ws_dir = self.ws.dir.to_string_lossy().to_string();
        let query = args.query.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<Vec<(String, String)>, String> {
            let okf_dir = crate::okf::get_okf_dir(&ws_dir);
            let pipelines_dir = okf_dir.join("pipelines");
            if !pipelines_dir.exists() {
                return Ok(Vec::new());
            }
            let query_lower = query.to_lowercase();
            let mut hits = Vec::new();
            for entry in walkdir::WalkDir::new(&pipelines_dir).into_iter().flatten() {
                if !entry.path().is_file() {
                    continue;
                }
                if entry.path().extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let rel = entry
                    .path()
                    .strip_prefix(&okf_dir)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .to_string();
                let Ok(content) = std::fs::read_to_string(entry.path()) else {
                    continue;
                };
                if content.to_lowercase().contains(&query_lower) {
                    let preview: String = content.lines().take(6).collect::<Vec<_>>().join("\n");
                    hits.push((rel, preview));
                }
            }
            Ok(hits)
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(hits) => {
                let summary = format!("找到 {} 条匹配的配方/排障记录", hits.len());
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), None, None, Some(elapsed), None);
                if hits.is_empty() {
                    Ok("未找到匹配的配方/排障记录。".to_string())
                } else {
                    let formatted: Vec<String> = hits.iter().map(|(path, preview)| format!("📄 {path}\n{preview}")).collect();
                    Ok(formatted.join("\n\n---\n\n"))
                }
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), None, None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}
