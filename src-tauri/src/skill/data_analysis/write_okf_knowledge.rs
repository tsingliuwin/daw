use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct WriteOkfKnowledgeArgs {
    category: String,
    name: String,
    heading: String,
    content: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    confirm_new: bool,
}

pub struct WriteOkfKnowledgeTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for WriteOkfKnowledgeTool {
    const NAME: &'static str = "write_okf_knowledge";
    type Error = ToolError;
    type Args = WriteOkfKnowledgeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "write_okf_knowledge".to_string(),
            description: "向本地 OKF 知识库写入或更新业务知识。当用户补充了字段释义、关联关系、排障经验，或一次分析确立/修正了业务认知时必须立即调用。类别选择：concepts=全局业务概念/公司背景；users=用户背景；tables/views=单表字段释义/关联；sources=数据源知识；selections=选表经验（哪类问题用哪些表）；pipelines/specific=清洗配方/排障。首次写入某个文件时建议提供 description（一句话用途说明）。新建文件时自动做相似度检测：若返回「疑似重复」候选，同一主题必须改用既有 name 写入既有文件；确属不同知识才传 confirm_new=true。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": format!("OKF 目录类别：{}", crate::okf::model::Category::prompt_list()) },
                    "name": { "type": "string", "description": "文件名（不含 .md 后缀）。优先复用知识库大纲中已有的同主题文件名" },
                    "heading": { "type": "string", "description": "要写入的标题，如：关联关系、异常排障记录、业务描述" },
                    "content": { "type": "string", "description": "要写入的内容（Markdown 格式）" },
                    "description": { "type": "string", "description": "（可选）该知识条目的一句话用途说明，便于检索与回顾；提供时会写入/更新文件 frontmatter 的 description 字段" },
                    "confirm_new": { "type": "boolean", "description": "（可选）新建文件被「疑似重复」守卫拦下后，确认与既有条目确属不同知识时传 true 强制新建" }
                },
                "required": ["category", "name", "heading", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okf");
        emit_tool_call(&self.window, &self.task_id, &call_id, "write_okf_knowledge", json!({
            "category": &args.category, "name": &args.name, "heading": &args.heading,
        }));

        let start = std::time::Instant::now();
        let cat = crate::okf::model::Category::from_str(&args.category)
            .ok_or_else(|| ToolError(format!("未知知识类别: {}", args.category)))?;
        let ws_path = self.ws.path.clone();
        let name = args.name.clone();
        let heading = args.heading.clone();
        let content = args.content.clone();
        let description = args.description.clone();

        // 防重守卫：即将新建文件时查同类目相似条目；命中且未显式确认 → 返回候选让 agent 决策。
        if !args.confirm_new {
            let guard_ws = ws_path.clone();
            let guard_name = name.clone();
            let guard_desc = description.clone();
            let candidates = tokio::task::spawn_blocking(move || {
                let okf = crate::okf::Okf::production();
                if okf.knowledge_exists(&guard_ws, cat, &guard_name) {
                    return Vec::new(); // 覆盖写入既有文件，放行
                }
                okf.find_similar(&guard_ws, cat, &guard_name, guard_desc.as_deref())
            }).await
            .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

            if !candidates.is_empty() {
                let scope_cn = cat.scope().label();
                let mut msg = format!(
                    "⚠️ 即将新建【{scope_cn}】{}/{}，但同类目下已有疑似重复条目：\n",
                    args.category, args.name
                );
                for c in &candidates {
                    let desc_part = if c.description.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", c.description)
                    };
                    msg.push_str(&format!("- {}（相似度 {:.2}）{}\n", c.name, c.score, desc_part));
                }
                msg.push_str("请二选一：\n1. 同一主题 → 改用既有 name 重新调用 write_okf_knowledge（可先 load_okf_knowledge 读全文再整合更新，互补内容写成同文件的不同板块）；\n2. 确属不同知识 → 重新调用并传 confirm_new=true。\n本次未写入。");
                let elapsed = start.elapsed().as_millis() as u64;
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    msg.clone(), None, None, Some(elapsed), None,
                );
                return Ok(msg);
            }
        }

        let result = tokio::task::spawn_blocking(move || {
            crate::okf::Okf::production()
                .write(&ws_path, cat, &name, &heading, &content, description.as_deref())
        }).await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(o) => {
                let scope_cn = o.scope.label();
                let created_str = if o.created { "（新建文件）" } else { "" };
                let summary = format!(
                    "已写入【{}】{}/{} 的「{}」板块{}（+{} −{} 行）",
                    scope_cn, args.category, args.name, args.heading, created_str,
                    o.added_lines, o.removed_lines
                );
                // 静默丢知识防线的第二道闸（第一道是写入层剥离回显标题）：
                // 净删多行且一行未增，多半不是有意的改写——当场提醒模型核对，
                // 而不是等下次复盘从 git 历史里考古。
                let warning = if !o.created && o.removed_lines >= 5 && o.added_lines == 0 {
                    format!(
                        "\n⚠️ 本次写入删除了 {} 行、新增 0 行：若不是有意清空/大幅精简该板块，请立即 load_okf_knowledge 核对文件现状。",
                        o.removed_lines
                    )
                } else {
                    String::new()
                };
                let summary = format!("{summary}{warning}");
                // detail=写入的内容（前端作"回执"展开显示），payload=结构化位置信息。
                let detail = Some(args.content.clone());
                let payload = serde_json::json!({
                    "scope": scope_cn,
                    "file": o.file_path.to_string_lossy(),
                    "created": o.created,
                    "addedLines": o.added_lines,
                    "removedLines": o.removed_lines,
                });
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), detail, Some(payload), Some(elapsed), None,
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
