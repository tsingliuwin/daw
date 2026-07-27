//! Rig Tool implementations. Each tool lives in its own file (Args + Tool
//! struct + `impl Tool`). The write tools share a driver in [`oa_shared`].
//!
//! (M1 ships 3 tools — enough to prove the loop:
//!   - `get_current_time`   — read-only, resolves relative-time expressions.
//!   - `get_leave_balance`  — read-only, queries an employee's annual-leave
//!                             balance via the OA backend.
//!   - `submit_leave`       — write, gated by the human-confirmation channel.

mod get_current_time;
mod get_leave_balance;
mod oa_shared;
mod submit_leave;

pub(crate) use get_current_time::GetCurrentTimeTool;
pub(crate) use get_leave_balance::GetLeaveBalanceTool;
pub(crate) use oa_shared::OaWriteShared;
pub(crate) use submit_leave::SubmitLeaveTool;
