use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

/// No-args marker (rig requires an Args type even for parameter-less tools).
#[derive(Deserialize, Serialize)]
pub(crate) struct GetCurrentTimeArgs {}

pub(crate) struct GetCurrentTimeTool {
    #[allow(dead_code)]
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for GetCurrentTimeTool {
    const NAME: &'static str = "get_current_time";
    type Error = ToolError;
    type Args = GetCurrentTimeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "get_current_time".to_string(),
            description: "获取当前日期和时间（含星期）。当用户提问涉及相对时间（「今天」「本周」「下周一」「本月」「上周」「最近三个月」「去年」等）时，**必须先调用本工具**确认当前时间，再据此计算时间范围。绝不能凭印象猜测当前日期。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("now");
        emit_tool_call(
            &self.window,
            &self.task_id,
            &call_id,
            "get_current_time",
            json!({}),
        );
        let start = std::time::Instant::now();

        let now_str = super::super::runner::current_datetime_str();
        let elapsed = start.elapsed().as_millis() as u64;
        emit_tool_result(
            &self.window,
            &self.task_id,
            &call_id,
            "ok",
            now_str.clone(),
            None,
            None,
            Some(elapsed),
            None,
        );
        Ok(now_str)
    }
}
