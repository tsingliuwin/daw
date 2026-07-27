//! Schema initialization + demo-data seeding for the local OA database.
//!
//! Idempotent: runs on every `LocalOaBackend::new()`. The schema is created
//! `IF NOT EXISTS`; demo employees are inserted only when the `employees`
//! table is empty (first launch), so the user's real data is never clobbered
//! on restart.

use rusqlite::Connection;

/// Create the OA tables if they don't yet exist.
pub fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS employees (
            id                 INTEGER PRIMARY KEY,
            name               TEXT NOT NULL,
            dept               TEXT NOT NULL,
            manager_id         INTEGER,
            leave_balance_days REAL NOT NULL DEFAULT 0
        )",
        [],
    )
    .map_err(|e| format!("create employees: {e}"))?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS leave_requests (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            employee_id INTEGER NOT NULL,
            start_date  TEXT NOT NULL,
            end_date    TEXT NOT NULL,
            days        REAL NOT NULL,
            reason      TEXT NOT NULL,
            status      TEXT NOT NULL DEFAULT 'pending',
            created_at  INTEGER NOT NULL,
            FOREIGN KEY(employee_id) REFERENCES employees(id)
        )",
        [],
    )
    .map_err(|e| format!("create leave_requests: {e}"))?;
    Ok(())
}

/// Seed a small demo org on first launch so the chat loop has someone to talk
/// about. Skipped if any employees already exist.
pub fn seed_demo_data(conn: &Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM employees", [], |r| r.get(0))
        .map_err(|e| format!("count employees: {e}"))?;
    if count > 0 {
        return Ok(());
    }

    // A tiny 3-person org: a manager + two reports, each with some leave balance.
    let demo: &[(i64, &str, &str, Option<i64>, f64)] = &[
        // (id, name, dept, manager_id, leave_balance_days)
        (1, "张伟", "管理层", None, 15.0),
        (2, "张三", "研发部", Some(1), 5.0),
        (3, "李四", "研发部", Some(1), 8.5),
    ];
    for (id, name, dept, manager_id, balance) in demo {
        conn.execute(
            "INSERT INTO employees (id, name, dept, manager_id, leave_balance_days) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![id, name, dept, manager_id, balance],
        )
        .map_err(|e| format!("seed employee {name}: {e}"))?;
    }
    tracing::info!(category = "oa", "seeded {} demo employees", demo.len());
    Ok(())
}
