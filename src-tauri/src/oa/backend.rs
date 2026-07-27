//! The swappable OA backend abstraction + a local SQLite demo implementation.
//!
//! Tools call the [`OaBackend`] trait; the concrete implementation decides
//! where the data lives. [`LocalOaBackend`] (M1) keeps everything in a SQLite
//! file under `~/.aioa/` — perfect for a demo loop. A future
//! `DingTalkBackend` / `FeishuBackend` / `WeComBackend` implements the same
//! trait against real HTTP APIs, and no tool code changes.
//!
//! ## Concurrency model
//!
//! `LocalOaBackend` wraps a single `rusqlite::Connection` in a `tokio::Mutex`
//! and runs every query in `spawn_blocking` (SQLite calls are blocking).
//! `busy_timeout = 5000` + `journal_mode = WAL` (set when the connection is
//! opened) keep concurrent readers/writers safe.

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::Connection;
use tokio::sync::Mutex;

use super::models::{Employee, LeaveRequest, LeaveStatus};
use crate::db;

/// The OA backend every tool talks to. Implementations decide where the data
/// lives (local SQLite in M1; a real OA API later).
#[async_trait]
pub trait OaBackend: Send + Sync {
    /// Resolve an employee by name (case-insensitive). Returns `Err` if the
    /// name is ambiguous (multiple matches) or not found — the agent surfaces
    /// this so it can ask the user to clarify.
    async fn find_employee(&self, name: &str) -> Result<Employee, String>;

    /// Read an employee's remaining annual-leave balance (days).
    async fn get_leave_balance(&self, employee_id: i64) -> Result<f64, String>;

    /// Submit a leave request: insert the row, deduct `days` from the balance,
    /// and return the created request (with its new id + status). This is a
    /// write operation — tools gate it behind the human-confirmation channel.
    async fn submit_leave(
        &self,
        employee_id: i64,
        start_date: &str,
        end_date: &str,
        days: f64,
        reason: &str,
    ) -> Result<LeaveRequest, String>;

    /// List a recent slice of a workspace's leave requests (newest-first),
    /// for the agent to answer "我最近请过假吗".
    ///
    /// M2 entry point: the matching `list_my_recent_leaves` tool isn't shipped
    /// in M1, but the backend capability is required by the trait contract so
    /// the local impl already satisfies it. Remove the allow when the tool lands.
    #[allow(dead_code)]
    async fn list_recent_leaves(&self, employee_id: i64, limit: usize) -> Result<Vec<LeaveRequest>, String>;
}

/// Local demo backend: SQLite at `~/.aioa/oa.db`.
pub struct LocalOaBackend {
    conn: Arc<Mutex<Connection>>,
}

impl LocalOaBackend {
    /// Open (and on first call, create + seed) the local OA database.
    pub fn new() -> Self {
        let conn = Self::open().expect("failed to open local OA database");
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    fn open() -> Result<Connection, String> {
        let mut path = db::get_aioa_dir()?;
        path.push("oa.db");
        let conn = Connection::open(&path).map_err(|e| format!("open oa.db: {e}"))?;
        let _ = conn.pragma_update(None, "busy_timeout", 5000);
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        super::seed::init_schema(&conn)?;
        super::seed::seed_demo_data(&conn)?;
        Ok(conn)
    }

    /// Helper: take the lock and hand the connection to a blocking closure on
    /// the blocking pool. Every trait method goes through this so the locking
    /// pattern stays uniform. The closure takes `&mut Connection` so it can
    /// open a transaction (e.g. `submit_leave` does balance-deduct + insert
    /// atomically).
    async fn with_conn<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.blocking_lock();
            f(&mut guard)
        })
        .await
        .map_err(|e| format!("oa backend task join error: {e}"))?
    }
}

#[async_trait]
impl OaBackend for LocalOaBackend {
    async fn find_employee(&self, name: &str) -> Result<Employee, String> {
        let name = name.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, name, dept, manager_id, leave_balance_days FROM employees WHERE name = ? COLLATE NOCASE")
                .map_err(|e| e.to_string())?;
            let mut rows = stmt
                .query_map([&name], |row| {
                    Ok(Employee {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        dept: row.get(2)?,
                        manager_id: row.get(3)?,
                        leave_balance_days: row.get(4)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            let first = rows.next();
            let second = rows.next();
            match (first, second) {
                (Some(Ok(emp)), None) => Ok(emp),
                (Some(Ok(_)), Some(_)) => {
                    Err(format!("员工「{}」存在重名，请补充部门或工号以澄清", name))
                }
                (Some(Err(e)), _) => Err(e.to_string()),
                (None, _) => Err(format!("未找到员工「{}」", name)),
            }
        })
        .await
    }

    async fn get_leave_balance(&self, employee_id: i64) -> Result<f64, String> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT leave_balance_days FROM employees WHERE id = ?",
                [employee_id],
                |r| r.get::<_, f64>(0),
            )
            .map_err(|e| format!("查询年假余额失败: {e}"))
        })
        .await
    }

    async fn submit_leave(
        &self,
        employee_id: i64,
        start_date: &str,
        end_date: &str,
        days: f64,
        reason: &str,
    ) -> Result<LeaveRequest, String> {
        let start_date = start_date.to_string();
        let end_date = end_date.to_string();
        let reason = reason.to_string();
        self.with_conn(move |conn| {
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            // Deduct balance first; if the balance is insufficient, abort before
            // inserting the request row.
            let balance: f64 = tx
                .query_row(
                    "SELECT leave_balance_days FROM employees WHERE id = ?",
                    [employee_id],
                    |r| r.get(0),
                )
                .map_err(|e| format!("查询员工失败: {e}"))?;
            if balance < days {
                return Err(format!(
                    "年假余额不足：当前剩余 {} 天，本次申请 {} 天",
                    balance, days
                ));
            }
            tx.execute(
                "UPDATE employees SET leave_balance_days = leave_balance_days - ? WHERE id = ?",
                rusqlite::params![days, employee_id],
            )
            .map_err(|e| e.to_string())?;
            let now = db::now_ms();
            tx.execute(
                "INSERT INTO leave_requests (employee_id, start_date, end_date, days, reason, status, created_at)
                 VALUES (?, ?, ?, ?, ?, 'pending', ?)",
                rusqlite::params![employee_id, start_date, end_date, days, reason, now],
            )
            .map_err(|e| e.to_string())?;
            let id = tx.last_insert_rowid();
            tx.commit().map_err(|e| e.to_string())?;
            Ok(LeaveRequest {
                id,
                employee_id,
                start_date,
                end_date,
                days,
                reason,
                status: LeaveStatus::Pending,
                created_at: now,
            })
        })
        .await
    }

    async fn list_recent_leaves(&self, employee_id: i64, limit: usize) -> Result<Vec<LeaveRequest>, String> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, employee_id, start_date, end_date, days, reason, status, created_at
                     FROM leave_requests WHERE employee_id = ?
                     ORDER BY created_at DESC LIMIT ?",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![employee_id, limit as i64], |row| {
                    let status_str: String = row.get(6)?;
                    Ok(LeaveRequest {
                        id: row.get(0)?,
                        employee_id: row.get(1)?,
                        start_date: row.get(2)?,
                        end_date: row.get(3)?,
                        days: row.get(4)?,
                        reason: row.get(5)?,
                        status: LeaveStatus::from_db_str(&status_str),
                        created_at: row.get(7)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for r in rows {
                if let Ok(req) = r {
                    out.push(req);
                }
            }
            Ok(out)
        })
        .await
    }
}
