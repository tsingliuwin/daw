use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

/// Args for `get_leave_balance`.
#[derive(Deserialize, Serialize)]
pub(crate) struct GetLeaveBalanceArgs {
    /// The employee's display name (e.g. "张三"). The backend resolves it; if
    /// the name is ambiguous (duplicates) the tool surfaces the error so the
    /// agent asks the user to clarify.
    employee_name: String,
}

pub(crate) struct GetLeaveBalanceTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for GetLeaveBalanceTool {
    const NAME: &'static str = "get_leave_balance";
    type Error = ToolError;
    type Args = GetLeaveBalanceArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "get_leave_balance".to_string(),
            description: "查询某位员工的剩余年假余额（单位：天）。参数 employee_name 是员工姓名。如果用户说「我还剩多少年假」「查一下 X 的年假」之类的话，调用本工具。返回的是真实余额，回答用户时必须使用这个真实数字，不能编造。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "employee_name": {
                        "type": "string",
                        "description": "员工姓名，例如 \"张三\""
                    }
                },
                "required": ["employee_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("leave");
        emit_tool_call(
            &self.window,
            &self.task_id,
            &call_id,
            "get_leave_balance",
            json!({ "employee_name": args.employee_name }),
        );
        let start = std::time::Instant::now();

        // 1. Resolve the employee by name (catches duplicates / unknown).
        let employee = match self.app_state.oa_backend.find_employee(&args.employee_name).await {
            Ok(e) => e,
            Err(msg) => {
                let elapsed = start.elapsed().as_millis() as u64;
                emit_tool_result(
                    &self.window,
                    &self.task_id,
                    &call_id,
                    "error",
                    msg.clone(),
                    None,
                    None,
                    Some(elapsed),
                    None,
                );
                return Err(ToolError(msg));
            }
        };

        // 2. Read the balance.
        match self.app_state.oa_backend.get_leave_balance(employee.id).await {
            Ok(balance) => {
                let elapsed = start.elapsed().as_millis() as u64;
                let summary = format!(
                    "{}（{}）剩余年假余额：{:.1} 天",
                    employee.name, employee.dept, balance
                );
                let payload = json!({
                    "employee": {
                        "id": employee.id,
                        "name": employee.name,
                        "dept": employee.dept,
                    },
                    "leaveBalanceDays": (balance as f64 * 10.0).round() / 10.0,
                });
                emit_tool_result(
                    &self.window,
                    &self.task_id,
                    &call_id,
                    "ok",
                    summary.clone(),
                    None,
                    Some(payload),
                    Some(elapsed),
                    None,
                );
                Ok(format!(
                    "{}（{}部门）剩余年假余额为 {:.1} 天",
                    employee.name, employee.dept, balance
                ))
            }
            Err(msg) => {
                let elapsed = start.elapsed().as_millis() as u64;
                emit_tool_result(
                    &self.window,
                    &self.task_id,
                    &call_id,
                    "error",
                    msg.clone(),
                    None,
                    None,
                    Some(elapsed),
                    None,
                );
                Err(ToolError(msg))
            }
        }
    }
}
