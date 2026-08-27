//! P0-B 定案后的验证套件（根因：ICU 加载前后 DuckDB 会话时区 UTC→+08 翻转，
//! 叠加 postgres_query 内层字面量由远端 +08 解释的路径劈叉；修复 = 连接创建
//! 时 SET TimeZone='Asia/Shanghai'，见 state.rs::create_workspace_conn）。
//!
//! 验证三件事：
//! 1. SET TimeZone 在未加载任何扩展时即生效（epoch 探针：同一朴素字面量的
//!    绝对时刻随会话时区移动 28800s）。
//! 2. 钉死能扛住 ICU 自动加载（执行 AT TIME ZONE 'Asia/Shanghai' 后不回退）。
//! 3. 钉死后视图路径（本地求值）与 postgres_query 内层（远端求值）对同一
//!    朴素字面量窗口返回一致数字（2025-08 与 2026-08 各验一次）。

use std::time::Instant;

fn epoch_of(conn: &duckdb::Connection, lit: &str) -> f64 {
    conn.query_row(&format!("SELECT epoch(TIMESTAMPTZ '{lit}')"), [], |r| r.get(0))
        .unwrap_or(f64::NAN)
}

fn main() {
    println!("=== P0-B 修复验证：会话时区钉死 ===\n");

    // --- 1/2. 纯净连接上的 SET 语义与 ICU 免疫 ---
    let fresh = duckdb::Connection::open_in_memory().unwrap();
    let default_tz: String = fresh.query_row("SELECT current_setting('TimeZone')", [], |r| r.get(0)).unwrap();
    let e_default = epoch_of(&fresh, "2025-07-31 16:00:00");
    let _ = fresh.execute_batch("SET TimeZone='Asia/Shanghai';");
    let e_pinned = epoch_of(&fresh, "2025-07-31 16:00:00");
    // 触发 ICU 自动加载（命名时区转换），再测钉死是否存活
    let icu: String = fresh
        .query_row("SELECT (TIMESTAMPTZ '2025-07-31 16:00:00') AT TIME ZONE 'Asia/Shanghai'", [], |r| r.get(0))
        .unwrap_or_else(|e| format!("ERROR {e}"));
    let tz_after_icu: String = fresh.query_row("SELECT current_setting('TimeZone')", [], |r| r.get(0)).unwrap();
    let e_after_icu = epoch_of(&fresh, "2025-07-31 16:00:00");
    println!("[纯净连接] 默认 TimeZone = {default_tz}");
    println!("[纯净连接] epoch(默认)   = {e_default}");
    println!("[纯净连接] epoch(钉死后) = {e_pinned}  （差 {}s，=28800 说明 SET 未加载扩展即生效）", e_default - e_pinned);
    println!("[纯净连接] AT TIME ZONE 结果 = {icu}");
    println!("[纯净连接] ICU 加载后 TimeZone = {tz_after_icu}，epoch = {e_after_icu}  （与钉死一致=免疫回退）\n");

    // --- 3. 双路径一致性（与应用 create_workspace_conn 相同的初始化序列）---
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let sqlite = rusqlite::Connection::open(format!("{home}/.daw/daw.db")).unwrap();
    let row: (String, i64, String, String, String, String) = sqlite
        .query_row(
            "SELECT host, port, database_name, username, password, ssl_mode FROM db_connections WHERE name = 'yantubi'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    let (host, port, dbname, user, password, ssl_mode) = row;
    let mut conn_str = format!("host={host} port={port} dbname={dbname} user={user} password={password}");
    if ssl_mode != "disable" {
        conn_str.push_str(&format!(" sslmode={ssl_mode}"));
    }

    let conn = duckdb::Connection::open_in_memory().unwrap();
    let _ = conn.execute_batch("PRAGMA memory_limit='4GB';\nPRAGMA threads=1;");
    let _ = conn.execute_batch("SET TimeZone='Asia/Shanghai';"); // ← 应用侧修复动作
    let _ = conn.execute_batch("LOAD postgres;");
    conn.execute(&format!("ATTACH '{}' AS db_yantubi (TYPE postgres);", conn_str.replace('\'', "''")), [])
        .unwrap();
    conn.execute_batch(
        "CREATE VIEW v_sale AS SELECT * FROM postgres_query('db_yantubi', 'SELECT * FROM \"default\".\"dws_sale_dept_order_detail_all_trade\"');",
    )
    .unwrap();
    let tz_now: String = conn.query_row("SELECT current_setting('TimeZone')", [], |r| r.get(0)).unwrap();
    println!("[应用路径] 初始化后 TimeZone = {tz_now}");

    let q = |sql: &str, n: usize| -> String {
        let t = Instant::now();
        match conn.query_row(sql, [], |r| {
            let mut parts = Vec::new();
            for i in 0..n {
                let v: String = match r.get_ref(i) {
                    Ok(duckdb::types::ValueRef::Null) => "NULL".into(),
                    Ok(duckdb::types::ValueRef::Int(i)) => i.to_string(),
                    Ok(duckdb::types::ValueRef::BigInt(i)) => i.to_string(),
                    Ok(duckdb::types::ValueRef::Double(d)) => d.to_string(),
                    Ok(duckdb::types::ValueRef::Text(s)) => String::from_utf8_lossy(s).to_string(),
                    Ok(duckdb::types::ValueRef::Decimal(d)) => d.to_string(),
                    _ => "?".into(),
                };
                parts.push(v);
            }
            Ok(parts.join(" | "))
        }) {
            Ok(s) => format!("{s}   （{}ms）", t.elapsed().as_millis()),
            Err(e) => format!("ERROR: {e}"),
        }
    };

    // 模拟会话中途出现命名时区转换（ICU 触发点），随后复查时区与窗口结果
    println!("[应用路径] ICU 触发查询: {}", q("SELECT COUNT(*) FROM v_sale WHERE (event_time AT TIME ZONE 'Asia/Shanghai') >= TIMESTAMPTZ '2025-01-01 00:00:00'", 1));
    let tz_mid: String = conn.query_row("SELECT current_setting('TimeZone')", [], |r| r.get(0)).unwrap();
    println!("[应用路径] ICU 触发后 TimeZone = {tz_mid}\n");

    for (label, remote, view) in [
        (
            "2025-08",
            "SELECT * FROM postgres_query('db_yantubi', 'SELECT COUNT(*) c, SUM(real_payment) s FROM \"default\".\"dws_sale_dept_order_detail_all_trade\" WHERE class_type_new_lv1=''总计'' AND scrm_dept_lv3_name=''郑州咨询部'' AND event_time >= ''2025-07-31 16:00:00'' AND event_time < ''2025-08-26 16:00:00''')",
            "SELECT COUNT(*) c, ROUND(SUM(real_payment),0) s FROM v_sale WHERE class_type_new_lv1='总计' AND scrm_dept_lv3_name='郑州咨询部' AND event_time >= '2025-07-31 16:00:00' AND event_time < '2025-08-26 16:00:00'",
        ),
        (
            "2026-08",
            "SELECT * FROM postgres_query('db_yantubi', 'SELECT COUNT(*) c, SUM(real_payment) s FROM \"default\".\"dws_sale_dept_order_detail_all_trade\" WHERE class_type_new_lv1=''总计'' AND scrm_dept_lv3_name=''郑州咨询部'' AND event_time >= ''2026-07-31 16:00:00'' AND event_time < ''2026-08-26 16:00:00''')",
            "SELECT COUNT(*) c, ROUND(SUM(real_payment),0) s FROM v_sale WHERE class_type_new_lv1='总计' AND scrm_dept_lv3_name='郑州咨询部' AND event_time >= '2026-07-31 16:00:00' AND event_time < '2026-08-26 16:00:00'",
        ),
    ] {
        println!("{label} 远程路径: {}", q(remote, 2));
        println!("{label} 视图路径: {}", q(view, 2));
    }

    println!("\n=== 验证结束：两路径数字一致 = P0-B 修复成立 ===");
}
