//! 外部数据源 ATTACH——把 postgres/mysql/sqlite 数据库挂载到 DuckDB 会话。
//!
//! 每个 DataSourceConfig 对应一个 DuckDB catalog 别名 `db_<safe_name>`，
//! 挂载后可用 `db_<name>.schema.table` 引用远程表。
//! 迁移时砍掉了 MaxCompute/sidecar 分支。

use crate::model::DataSourceConfig;

/// 把连接名规整成 DuckDB catalog 别名 `db_<safe>`（仅字母数字下划线）。
pub fn workspace_attach_alias(name: &str) -> String {
    let safe = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect::<String>();
    format!("db_{safe}")
}

/// 构造 ATTACH SQL（密码明文拼接，因为 DuckDB ATTACH 不接受参数绑定）。
pub fn build_attach_sql(r: &DataSourceConfig, alias: &str) -> String {
    if r.db_type == "sqlite" {
        let path = r.database_name.replace('\'', "''");
        format!("ATTACH '{path}' AS {alias} (TYPE sqlite);")
    } else if r.db_type == "postgres" {
        let mut conn_str = format!(
            "host={} port={} dbname={} user={} password={}",
            r.host, r.port, r.database_name, r.username, r.password
        );
        if r.ssl_mode != "disable" {
            conn_str.push_str(&format!(" sslmode={}", r.ssl_mode));
        }
        // READ_ONLY: 联邦查询模式不写远程。
        format!("ATTACH '{}' AS {alias} (TYPE postgres);", conn_str)
    } else {
        // mysql
        format!(
            "ATTACH 'host={} port={} database={} user={} password={}' AS {alias} (TYPE mysql);",
            r.host, r.port, r.database_name, r.username, r.password
        )
    }
}

/// ATTACH 单个数据源。先 LOAD 扩展（失败则 INSTALL + LOAD），再 ATTACH。
pub fn attach_one(conn: &duckdb::Connection, r: &DataSourceConfig) -> Result<(), String> {
    let load_sql = format!("LOAD {};", r.db_type);
    if conn.execute(&load_sql, []).is_err() {
        let install_sql = format!("INSTALL {};", r.db_type);
        let _ = conn.execute(&install_sql, []);
        conn.execute(&load_sql, [])
            .map_err(|e| format!("加载 {} 驱动失败: {e}", r.db_type))?;
    }

    let alias = workspace_attach_alias(&r.name);
    let attach_sql = build_attach_sql(r, &alias);
    conn.execute(&attach_sql, [])
        .map_err(|e| format!("连接数据源 {} 失败: {e}", r.name))?;
    tracing::info!(category = "link", "ATTACH 数据源: {} AS {}", r.name, alias);
    Ok(())
}

/// 遍历全部数据源逐个 ATTACH，单个失败只 warn 不中断。
pub fn attach_all(conn: &duckdb::Connection, sources: &[DataSourceConfig]) -> Result<(), String> {
    for r in sources {
        if let Err(e) = attach_one(conn, r) {
            tracing::warn!(category = "link", "ATTACH 数据源 {} 失败: {e}", r.name);
        }
    }
    Ok(())
}

/// DETACH 单个数据源（unlink 时调用）。
pub fn detach_one(conn: &duckdb::Connection, name: &str) -> Result<(), String> {
    let alias = workspace_attach_alias(name);
    let sql = format!("DETACH {};", alias);
    conn.execute(&sql, [])
        .map_err(|e| format!("DETACH 数据源 {} 失败: {e}", name))?;
    tracing::info!(category = "link", "DETACH 数据源: {} ({})", name, alias);
    Ok(())
}

/// 构造 postgres_query 函数用的连接串（和 ATTACH 的格式相同）。
/// 用于 Hologres 等兼容 PG 协议但 catalog 元数据扫描不兼容的数据库。
#[allow(dead_code)]
pub fn build_pg_conn_str(r: &DataSourceConfig) -> String {
    let mut conn_str = format!(
        "host={} port={} dbname={} user={} password={}",
        r.host, r.port, r.database_name, r.username, r.password
    );
    if r.ssl_mode != "disable" {
        conn_str.push_str(&format!(" sslmode={}", r.ssl_mode));
    }
    conn_str
}
