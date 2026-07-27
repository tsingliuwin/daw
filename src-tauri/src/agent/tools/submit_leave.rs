use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::oa_shared::OaWriteShared;

/// Args for `submit_leave`.
#[derive(Deserialize, Serialize)]
pub(crate) struct SubmitLeaveArgs {
    /// Employee's display name. Resolved to an id before submission; ambiguity
    /// surfaces as an error so the agent asks for clarification.
    employee_name: String,
    /// ISO date `YYYY-MM-DD`, inclusive.
    start_date: String,
    /// ISO date `YYYY-MM-DD`, inclusive.
    end_date: String,
    reason: String,
}

pub(crate) struct SubmitLeaveTool {
    pub(crate) shared: OaWriteShared,
}

impl Tool for SubmitLeaveTool {
    const NAME: &'static str = "submit_leave";
    type Error = ToolError;
    type Args = SubmitLeaveArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "submit_leave".to_string(),
            description: "提交一条请假申请（写操作）。会扣减对应员工的年假余额并生成一条待审批的请假单。涉及相对时间（「下周一」「本周三」等）时，**必须先调用 get_current_time 确认当前日期**，再据此把相对时间换算成 start_date / end_date（YYYY-MM-DD 格式）。提交前请确认对象（哪个员工）、日期、事由都清楚无误。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "employee_name": {
                        "type": "string",
                        "description": "员工姓名，例如 \"张三\""
                    },
                    "start_date": {
                        "type": "string",
                        "description": "请假开始日期，YYYY-MM-DD 格式，例如 \"2026-08-03\""
                    },
                    "end_date": {
                        "type": "string",
                        "description": "请假结束日期（含当天），YYYY-MM-DD 格式，例如 \"2026-08-05\""
                    },
                    "reason": {
                        "type": "string",
                        "description": "请假事由，例如 \"家里有事\""
                    }
                },
                "required": ["employee_name", "start_date", "end_date", "reason"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Resolve the employee BEFORE entering the confirmation gate, so the
        // awaiting UI can show a concrete name/dept and so a bad name fails fast
        // without making the user confirm an ambiguous request.
        let employee = match self.shared.app_state.oa_backend.find_employee(&args.employee_name).await {
            Ok(e) => e,
            Err(msg) => {
                // Emit a synthetic tool_call + error result so the transcript
                // shows why this attempt failed (mirrors the gated path's shape).
                use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
                let call_id = next_tool_id("leave");
                emit_tool_call(
                    &self.shared.window,
                    &self.shared.task_id,
                    &call_id,
                    "submit_leave",
                    json!({ "employee_name": args.employee_name }),
                );
                emit_tool_result(
                    &self.shared.window,
                    &self.shared.task_id,
                    &call_id,
                    "error",
                    msg.clone(),
                    None,
                    None,
                    None,
                    None,
                );
                return Err(ToolError(msg));
            }
        };

        // Compute calendar days (end - start + 1). Validate the date format
        // first so a malformed input fails before the confirmation gate rather
        // than after the user approves.
        let days = match compute_days(&args.start_date, &args.end_date) {
            Ok(d) => d,
            Err(msg) => {
                use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
                let call_id = next_tool_id("leave");
                emit_tool_call(
                    &self.shared.window,
                    &self.shared.task_id,
                    &call_id,
                    "submit_leave",
                    json!({
                        "employee_name": args.employee_name,
                        "start_date": args.start_date,
                        "end_date": args.end_date,
                    }),
                );
                emit_tool_result(
                    &self.shared.window,
                    &self.shared.task_id,
                    &call_id,
                    "error",
                    msg.clone(),
                    None,
                    None,
                    None,
                    None,
                );
                return Err(ToolError(msg));
            }
        };

        let summary_pending = format!(
            "提交 {}（{}）的请假申请：{} → {} 共 {:.0} 天",
            employee.name, employee.dept, args.start_date, args.end_date, days
        );
        let detail = format!(
            "将提交「{}（{}部门）」的请假单：\n- 日期：{} 至 {}（含，共 {:.0} 天）\n- 事由：{}\n提交后将扣减 {:.0} 天年假余额，并生成一条待审批记录。",
            employee.name, employee.dept, args.start_date, args.end_date, days, args.reason, days
        );

        let backend = self.shared.app_state.oa_backend.clone();
        let employee_id = employee.id;
        let start_date = args.start_date.clone();
        let end_date = args.end_date.clone();
        let reason = args.reason.clone();
        let emp_name = employee.name.clone();
        let emp_dept = employee.dept.clone();

        self.shared
            .run(
                "submit_leave",
                "leave",
                json!({
                    "employee_name": args.employee_name,
                    "start_date": args.start_date,
                    "end_date": args.end_date,
                    "reason": args.reason,
                }),
                summary_pending,
                detail,
                move || {
                    let backend = backend.clone();
                    let start_date = start_date.clone();
                    let end_date = end_date.clone();
                    let reason = reason.clone();
                    let emp_name = emp_name.clone();
                    let emp_dept = emp_dept.clone();
                    async move {
                        let req = backend
                            .submit_leave(employee_id, &start_date, &end_date, days, &reason)
                            .await?;
                        let summary = format!(
                            "{}（{}）的请假申请已提交：{} → {} 共 {:.0} 天，状态：待审批",
                            emp_name, emp_dept, req.start_date, req.end_date, req.days
                        );
                        let payload = json!({
                            "leaveRequest": {
                                "id": req.id,
                                "employeeId": req.employee_id,
                                "startDate": req.start_date,
                                "endDate": req.end_date,
                                "days": req.days,
                                "reason": req.reason,
                                "status": req.status.as_str(),
                                "createdAt": req.created_at,
                            }
                        });
                        Ok((summary, payload))
                    }
                },
            )
            .await
    }
}

/// Compute calendar days between two `YYYY-MM-DD` dates (inclusive).
fn compute_days(start: &str, end: &str) -> Result<f64, String> {
    let s = parse_date(start).ok_or_else(|| format!("开始日期格式不正确（应为 YYYY-MM-DD）：{start}"))?;
    let e = parse_date(end).ok_or_else(|| format!("结束日期格式不正确（应为 YYYY-MM-DD）：{end}"))?;
    if e < s {
        return Err(format!("结束日期 {end} 早于开始日期 {start}"));
    }
    Ok((e - s + 1) as f64)
}

/// Parse a `YYYY-MM-DD` string into a day number (proleptic Gregorian), or
/// `None` if the format is wrong. Std-only (no chrono dep).
fn parse_date(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i64 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// Howard Hinnant's `days_from_civil`: (y, m, d) -> proleptic Gregorian day
/// number. Matches `runner::civil_from_days` (the inverse).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as u64 + 2) / 5 + d as u64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe as i64 - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_single_day() {
        assert_eq!(compute_days("2026-08-03", "2026-08-03").unwrap(), 1.0);
    }

    #[test]
    fn days_three_days() {
        assert_eq!(compute_days("2026-08-03", "2026-08-05").unwrap(), 3.0);
    }

    #[test]
    fn days_across_month() {
        assert_eq!(compute_days("2026-08-30", "2026-09-02").unwrap(), 4.0);
    }

    #[test]
    fn days_rejects_inverted_range() {
        assert!(compute_days("2026-08-05", "2026-08-03").is_err());
    }

    #[test]
    fn days_rejects_bad_format() {
        assert!(compute_days("2026/08/03", "2026-08-05").is_err());
        assert!(compute_days("2026-8-3", "2026-08-05").is_err());
    }
}
