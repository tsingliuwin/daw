//! 自动找有权限的表，测 catalog vs postgres_query 的 COUNT/WHERE 下推。

use std::time::Instant;

fn main() {
    println!("=== Hologres catalog vs postgres_query 下推测试 ===\n");

    let conn = duckdb::Connection::open_in_memory().unwrap();
    let _ = conn.execute_batch("PRAGMA memory_limit='4GB';\nPRAGMA threads=1;");
    if conn.execute("LOAD ducklake;", []).is_err() { let _ = conn.execute("INSTALL ducklake;", []); conn.execute("LOAD ducklake;", []).unwrap(); }
    if conn.execute("LOAD sqlite;", []).is_err() { let _ = conn.execute("INSTALL sqlite;", []); conn.execute("LOAD sqlite;", []).unwrap(); }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let db_path = format!("{home}/.aioa/aioa.db");
    let sqlite = rusqlite::Connection::open(&db_path).unwrap();
    let lake_dir = format!("{home}/.aioa/DefaultProject");
    let _ = std::fs::create_dir_all(format!("{lake_dir}/.lake/lake_data/"));
    let catalog_str = format!("{lake_dir}/.lake/lake.sqlite").replace('\\', "/");
    let data_str = format!("{lake_dir}/.lake/lake_data/").replace('\\', "/");
    let _ = conn.execute(&format!("ATTACH 'ducklake:sqlite:{catalog_str}' AS lake (DATA_PATH '{data_str}');"), []);
    let _ = conn.execute("USE lake;", []);
    let _ = conn.execute("SET sqlite_all_varchar=true;", []);

    // 对每个 postgres 数据源
    let mut stmt = sqlite.prepare("SELECT id, name FROM db_connections WHERE db_type = 'postgres' ORDER BY created_at").unwrap();
    let conns: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().filter_map(|r| r.ok()).collect();

    for (conn_id, name) in &conns {
        println!("\n============================================================");
        println!("数据源: {name}");
        println!("============================================================");

        let row: (String, i64, String, String, String, String) = sqlite
            .query_row("SELECT host, port, database_name, username, password, ssl_mode FROM db_connections WHERE id = ?", [&conn_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
            .unwrap();
        let (host, port, dbname, user, password, ssl_mode) = row;
        let mut conn_str = format!("host={host} port={port} dbname={dbname} user={user} password={password}");
        if ssl_mode != "disable" { conn_str.push_str(&format!(" sslmode={ssl_mode}")); }
        let alias = format!("db_{}", name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect::<String>());

        println!("ATTACH...");
        if let Err(e) = conn.execute(&format!("ATTACH '{}' AS {alias} (TYPE postgres);", conn_str), []) {
            println!("ATTACH 失败: {e}"); continue;
        }

        // 用 postgres_query 查所有表（含 relkind）
        println!("查表列表...");
        let list_sql = format!(
            "SELECT * FROM postgres_query('{alias}', 'SELECT n.nspname, c.relname, c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind IN (''r'', ''f'') AND n.nspname NOT IN (''pg_catalog'', ''information_schema'', ''pg_toast'', ''hologres'', ''hg_recyclebin'') ORDER BY n.nspname, c.relname LIMIT 20')"
        );
        let tables: Vec<(String, String, String)> = match conn.prepare(&list_sql) {
            Ok(mut s) => s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect()),
            Err(e) => { println!("查表失败: {e}"); continue; }
        }.unwrap_or_default();

        // 找一张 catalog LIMIT 5 能成功的表
        let mut test_table: Option<(String, String, String)> = None;
        for (schema, table, relkind) in &tables {
            println!("  尝试 catalog: {schema}.{table} (relkind={relkind})...");
            let sql = format!("SELECT * FROM {alias}.{schema}.\"{table}\" LIMIT 1");
            match conn.execute(&sql, []) {
                Ok(_) => {
                    println!("    -> 成功！");
                    test_table = Some((schema.clone(), table.clone(), relkind.clone()));
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("pg_namespace") || msg.contains("BEGIN TRANSACTION") {
                        println!("    -> 失败（pg_namespace 扫描），此数据源 catalog 路径不可用");
                        break;
                    }
                    println!("    -> 跳过（权限/其他）");
                }
            }
        }

        let Some((schema, table, relkind)) = test_table else {
            println!("没有找到 catalog 可查的表，跳过此数据源");
            continue;
        };

        println!("\n选定测试表: {schema}.{table} (relkind={relkind})\n");

        // 测试 1: catalog LIMIT 5
        println!("--- catalog LIMIT 5 ---");
        let t = Instant::now();
        match conn.execute(&format!("SELECT * FROM {alias}.{schema}.\"{table}\" LIMIT 5"), []) {
            Ok(_) => println!("    OK {}ms", t.elapsed().as_millis()),
            Err(e) => println!("    FAIL: {e}"),
        }

        // 测试 2: postgres_query LIMIT 5
        println!("--- postgres_query LIMIT 5 ---");
        let t = Instant::now();
        match conn.execute(&format!("SELECT * FROM postgres_query('{alias}', 'SELECT * FROM \"{schema}\".\"{table}\" LIMIT 5')"), []) {
            Ok(_) => println!("    OK {}ms", t.elapsed().as_millis()),
            Err(e) => println!("    FAIL: {e}"),
        }

        // 测试 3: catalog COUNT(*)
        println!("--- catalog COUNT(*) ---");
        let t = Instant::now();
        match conn.query_row(&format!("SELECT COUNT(*) FROM {alias}.{schema}.\"{table}\""), [], |r| r.get::<_, i64>(0)) {
            Ok(c) => println!("    OK count={} {}ms", c, t.elapsed().as_millis()),
            Err(e) => println!("    FAIL: {e}"),
        }

        // 测试 4: postgres_query COUNT(*)
        println!("--- postgres_query COUNT(*) ---");
        let t = Instant::now();
        match conn.query_row(&format!("SELECT * FROM postgres_query('{alias}', 'SELECT COUNT(*) FROM \"{schema}\".\"{table}\"')"), [], |r| r.get::<_, i64>(0)) {
            Ok(c) => println!("    OK count={} {}ms", c, t.elapsed().as_millis()),
            Err(e) => println!("    FAIL: {e}"),
        }

        // 测试 5: catalog DESCRIBE（看列名）
        println!("--- catalog DESCRIBE ---");
        let t = Instant::now();
        match conn.prepare(&format!("SELECT * FROM {alias}.{schema}.\"{table}\" LIMIT 0")) {
            Ok(s) => {
                let cols: Vec<String> = s.schema().fields().iter().map(|f| f.name().clone()).collect();
                println!("    OK cols={} {}ms", cols.join(", "), t.elapsed().as_millis());
            }
            Err(e) => println!("    FAIL: {e}"),
        }

        // 测试 6: 用第一列做 WHERE + COUNT
        let first_col = {
            match conn.prepare(&format!("SELECT * FROM {alias}.{schema}.\"{table}\" LIMIT 0")) {
                Ok(s) => s.schema().fields().first().map(|f| f.name().clone()),
                Err(_) => None,
            }
        };
        if let Some(col) = first_col {
            println!("--- catalog WHERE + COUNT (col={col}) ---");
            let t = Instant::now();
            match conn.query_row(&format!("SELECT COUNT(*) FROM {alias}.{schema}.\"{table}\" WHERE \"{col}\" IS NOT NULL"), [], |r| r.get::<_, i64>(0)) {
                Ok(c) => println!("    OK count={} {}ms", c, t.elapsed().as_millis()),
                Err(e) => println!("    FAIL: {e}"),
            }

            println!("--- postgres_query WHERE + COUNT ---");
            let t = Instant::now();
            match conn.query_row(&format!("SELECT * FROM postgres_query('{alias}', 'SELECT COUNT(*) FROM \"{schema}\".\"{table}\" WHERE \"{col}\" IS NOT NULL')"), [], |r| r.get::<_, i64>(0)) {
                Ok(c) => println!("    OK count={} {}ms", c, t.elapsed().as_millis()),
                Err(e) => println!("    FAIL: {e}"),
            }
        }
    }

    println!("\n=== 测试完成 ===");
    println!("分析：");
    println!("  - catalog COUNT vs postgres_query COUNT 耗时差不多 -> 下推了");
    println!("  - catalog COUNT 比 postgres_query COUNT 慢很多 -> 没下推，拉全表");
    println!("  - catalog LIMIT 比 postgres_query LIMIT 慢很多 -> 没下推 LIMIT");
}
