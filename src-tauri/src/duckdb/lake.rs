//! DuckLake lakehouse connection management.
//!
//! Each workspace owns a DuckLake stored under its directory:
//!   `<workspace>/.lake/lake.sqlite` — the catalog (SQLite backend, avoids
//!     DuckDB's ART-index corruption bugs on crash; see duckdb #18505/#21468)
//!   `<workspace>/.lake/lake_data/`  — the data (parquet files for materialized tables)
//!
//! The DuckDB *session* connection is in-memory; DuckLake is the sole persistent
//! layer for views and tables. After [`attach_workspace_lake`], the lake is the
//! default catalog, so unqualified names resolve there (e.g. `FROM v_orders`).
//!
//! `duckdb_tables() WHERE database_name = 'lake'` 谓词下推只查 lake catalog
//! 元数据，不枚举 ATTACH 的远程 catalog（Hologres），不触发元数据扫描。

use std::path::Path;

use duckdb::Connection;

/// Hidden directory under a workspace that holds ALL DuckDB/DuckLake artifacts.
pub const LAKE_DIR: &str = ".lake";
/// DuckLake catalog file (SQLite backend).
pub const CATALOG_FILE: &str = "lake.sqlite";
/// DuckLake parquet data directory within [`LAKE_DIR`].
pub const DATA_DIR: &str = "lake_data";

/// Make sure the `ducklake` extension is loaded on `conn`. Idempotent.
/// 首次运行需联网下载 ducklake + sqlite 扩展。
pub fn ensure_ducklake_loaded(conn: &Connection) -> Result<(), String> {
    if conn.execute("LOAD ducklake;", []).is_err() {
        if let Err(e) = conn.execute("INSTALL ducklake;", []) {
            tracing::warn!(category = "duckdb", "INSTALL ducklake failed: {e}");
        }
        conn.execute("LOAD ducklake;", []).map_err(|e| {
            format!("无法加载 ducklake 扩展。首次运行需要联网下载该扩展。\n原始错误: {e}")
        })?;
    }

    // sqlite 扩展：DuckLake 用 SQLite 作为 catalog 后端（ducklake:sqlite:）。
    if conn.execute("LOAD sqlite;", []).is_err() {
        if let Err(e) = conn.execute("INSTALL sqlite;", []) {
            tracing::warn!(category = "duckdb", "INSTALL sqlite failed: {e}");
        }
        conn.execute("LOAD sqlite;", [])
            .map_err(|e| format!("无法加载 sqlite 扩展: {e}"))?;
    }
    Ok(())
}

/// Resolve `<workspace>/.lake/lake.sqlite` and `<workspace>/.lake/lake_data/`,
/// creating the directories if they do not yet exist.
pub fn ensure_lake_paths(ws_dir: &Path) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    std::fs::create_dir_all(ws_dir)
        .map_err(|e| format!("无法创建工作区目录: {e}"))?;
    let lake_dir = ws_dir.join(LAKE_DIR);
    std::fs::create_dir_all(&lake_dir)
        .map_err(|e| format!("无法创建 lake 目录: {e}"))?;
    let data_dir = lake_dir.join(DATA_DIR);
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("无法创建 lake 数据目录: {e}"))?;
    Ok((lake_dir.join(CATALOG_FILE), data_dir))
}

/// ATTACH a workspace's DuckLake and set it as the default catalog (`USE lake`).
///
/// Creates the catalog + data dir on first use. After this returns, every
/// unqualified table/view reference resolves inside the lake.
///
/// **Crash recovery:** if the previous process was killed mid-write, the
/// catalog's WAL can be out of sync. We detect that and drop the stale WAL
/// (catalog file is intact via checkpoint, no data loss). Only if the catalog
/// itself is corrupt do we rebuild an empty one.
pub fn attach_workspace_lake(conn: &Connection, ws_dir: &Path) -> Result<(), String> {
    let (catalog, data_dir) = ensure_lake_paths(ws_dir)?;
    // DuckDB paths inside SQL must use forward slashes (Windows backslashes break).
    let catalog_str = catalog.to_string_lossy().replace('\\', "/");
    let wal_str = format!("{catalog_str}.wal");
    let data_str = format!("{}/", data_dir.to_string_lossy().replace('\\', "/"));
    let sql = format!("ATTACH 'ducklake:sqlite:{catalog_str}' AS lake (DATA_PATH '{data_str}');");

    if let Err(first_err) = conn.execute(&sql, []) {
        let msg = first_err.to_string();
        // DuckLake snapshot/data inconsistency — NOT a WAL issue. Deleting the
        // WAL or rebuilding the lake would silently destroy already-committed
        // views/tables (the "索引有、沉淀无" class of bug). Surface it instead
        // so the user can investigate rather than losing data silently.
        if msg.contains("iteration does not match") || msg.contains("checkpoint iteration") {
            return Err(format!(
                "DuckLake catalog 快照不一致（可能数据文件缺失）: {msg}。已保留 catalog 未做破坏性恢复，请检查 {} 与 lake_data 目录。",
                catalog.display()
            ));
        }
        // SQLite catalog WAL corrupt (e.g. torn write after a crash). The
        // catalog main file is checkpoint-intact, so dropping the stale WAL
        // loses no committed data — safe to retry.
        if !msg.contains("WAL") {
            return Err(format!("ATTACH ducklake 失败: {msg}"));
        }
        tracing::warn!(category = "duckdb", "ducklake WAL mismatch after crash, attempting WAL-only recovery: {msg}");
        let _ = std::fs::remove_file(&wal_str);
        if conn.execute(&sql, []).is_ok() {
            tracing::info!(category = "duckdb", "ducklake recovered via WAL drop (catalog intact)");
        } else {
            // 最后兜底：catalog 本身损坏，重建空 lake。这是破坏性的（已提交
            // 的视图/表将丢失），显眼告警让用户知情。
            tracing::error!(category = "duckdb", "ducklake catalog corrupt, rebuilding EMPTY lake store (existing views/tables will be LOST)");
            let _ = std::fs::remove_file(&catalog);
            let _ = std::fs::remove_dir_all(&data_dir);
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("无法重建 lake 数据目录: {e}"))?;
            conn.execute(&sql, [])
                .map_err(|e| format!("ATTACH ducklake 失败（已尝试重建 lake store）: {e}"))?;
        }
    }

    conn.execute("USE lake;", [])
        .map_err(|e| format!("USE lake 失败: {e}"))?;

    // SQLite columns are dynamically typed. Loading every column as VARCHAR
    // sidesteps type validation issues. Session-level setting.
    conn.execute("SET sqlite_all_varchar=true;", [])
        .map_err(|e| format!("SET sqlite_all_varchar 失败: {e}"))?;
    Ok(())
}
