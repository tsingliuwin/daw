//! 基座内置工具——不属于任何 skill，所有 skill 都能用的通用能力。
//!
//! 当前：get_current_time（解析相对时间）。工具实例在 runner 的 build_tools 里
//! 用具体类型构造（rig Tool trait 非 dyn-compatible，无法 Box<dyn Tool>）。
//! 这个模块只放工具的结构体定义和 impl。

use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use crate::agent::error::ToolError;
use crate::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::agent::runner::current_datetime_str;
use crate::state::AppState;

/// No-args marker (rig requires an Args type even for parameter-less tools).
#[derive(Deserialize, Serialize)]
pub struct GetCurrentTimeArgs {}

pub struct GetCurrentTimeTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for GetCurrentTimeTool {
    const NAME: &'static str = "get_current_time";
    type Error = ToolError;
    type Args = GetCurrentTimeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "get_current_time".to_string(),
            description: "获取当前日期和时间（含星期与时分秒）。系统已在用户消息开头的 <runtime_context> 块注入当前日期，相对时间计算一般直接使用即可；需要时分秒精度或核对时间时再调用本工具。".to_string(),
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

        let now_str = current_datetime_str();
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
