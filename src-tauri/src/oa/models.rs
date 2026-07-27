//! OA domain entities. The wire format for tool payloads — keep
//! `src/lib/types.ts` (the OA payload shapes) in sync when changing.
//!
//! M1 ships `Employee` + `LeaveRequest` (the demo loop: query balance → submit
//! leave). `Reimbursement` / `Approval` are sketched as comments only; their
//! models + backend methods land in later iterations.

use serde::{Deserialize, Serialize};

/// One employee. Backs the demo dataset seeded on first launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Employee {
    /// Stable id (matches the `employees.id` SQLite column).
    pub id: i64,
    pub name: String,
    pub dept: String,
    /// Manager's employee id (for approval routing). `None` = top of the tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manager_id: Option<i64>,
    /// Remaining annual-leave balance in days. Mutated by `submit_leave`.
    pub leave_balance_days: f64,
}

/// Lifecycle of a submitted leave request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LeaveStatus {
    /// Submitted, awaiting the manager's approval.
    Pending,
    /// Manager approved; leave balance already deducted.
    Approved,
    /// Manager (or employee) rejected/cancelled; balance restored.
    Rejected,
}

impl LeaveStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
    /// Parse from the string stored in the `leave_requests.status` column.
    ///
    /// Actually called by `LocalOaBackend::list_recent_leaves` (backend.rs), but
    /// that call site lives behind the M2-only trait method, so the dead-code
    /// lint can't see through it. Kept on purpose.
    #[allow(dead_code)]
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            _ => Self::Pending,
        }
    }
}

/// One submitted leave request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveRequest {
    pub id: i64,
    pub employee_id: i64,
    /// ISO date `YYYY-MM-DD`.
    pub start_date: String,
    /// ISO date `YYYY-MM-DD` (inclusive).
    pub end_date: String,
    /// Calendar days covered (end - start + 1). Stored for convenience.
    pub days: f64,
    pub reason: String,
    pub status: LeaveStatus,
    /// Unix-ms timestamp of submission.
    pub created_at: i64,
}

// TODO(M2): Reimbursement { id, employee_id, amount, category, receipts, status, ... }
// TODO(M2): Approval    { id, request_type, request_id, approver_id, decision, ... }
