use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::lake;
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ListTablesArgs {}

pub struct ListTablesTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for ListTablesTool {
    const NAME: &'static str = "list_tables";
    type Error = ToolError;
    type Args = ListTablesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_tables".to_string(),
            description: "列出当前已注册的表和视图（本地 DuckLake catalog）。远程表通过 list_remote_tables 发现 + register_table 注册后才出现。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("list");
        emit_tool_call(&self.window, &self.task_id, &call_id, "list_tables", json!({}));

        let start = std::time::Instant::now();

        // 直接读 DuckLake 的 SQLite catalog 文件（lake.sqlite），
        // 完全绕过 DuckDB，不触发 ATTACH 的远程 catalog 元数据扫描。
        // duckdb_tables()/SHOW TABLES/information_schema 都会枚举所有 ATTACH 的
        // catalog（包括 Hologres），触发 postgres 扩展的元数据扫描报错。
        let ws_dir = self.app_state.workspace_dir.lock().await.clone();
        let tables_res = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
            list_tables_from_lake_catalog(&ws_dir)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("数据库查询失败: {e}"))));

        let elapsed = start.elapsed().as_millis() as u64;
        match tables_res {
            Ok(tables) => {
                let summary = if tables.is_empty() {
                    "当前没有已注册的表。请先调用 list_connections 和 list_remote_tables 发现并注册表。".to_string()
                } else {
                    format!("已注册 {} 个表/视图: {}", tables.len(), tables.join(", "))
                };
                let result = if tables.is_empty() {
                    None
                } else {
                    Some(tables.iter().map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n"))
                };
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, None, Some(elapsed), result,
                );
                Ok(format!("当前已注册的表/视图: {}", tables.join("; ")))
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}

/// 直接读 DuckLake 的 SQLite catalog 文件列出表/视图名。
/// DuckLake catalog 是 SQLite 后端，表/视图定义存在 lake.sqlite 里。
/// 绕过 DuckDB 完全不触发远程 catalog 元数据扫描。
pub fn list_tables_from_lake_catalog(ws_dir: &std::path::Path) -> Result<Vec<String>, String> {
    let catalog_path = ws_dir.join(lake::LAKE_DIR).join(lake::CATALOG_FILE);
    if !catalog_path.exists() {
        return Ok(Vec::new());
    }
    let conn = rusqlite::Connection::open(&catalog_path)
        .map_err(|e| format!("打开 DuckLake catalog 失败: {e}"))?;

    // DuckLake 在 SQLite catalog 里维护元数据表。表和视图名存在 tags 表或
    // duckdb_table_entries / duckdb_view_entries 表里（取决于 DuckLake 版本）。
    // 尝试多种 schema 兼容。
    let tables = try_list_ducklake_tables(&conn).unwrap_or_default();
    let mut result: Vec<String> = tables.into_iter().collect();
    result.sort();
    Ok(result)
}

/// 尝试从 DuckLake catalog 的各种可能表里读表/视图名。
fn try_list_ducklake_tables(conn: &rusqlite::Connection) -> Result<Vec<String>, String> {
    // 先看 catalog 里有哪些表
    let table_names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .map_err(|e| e.to_string())?
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    tracing::info!(category = "duckdb", "DuckLake catalog tables: {:?}", table_names);

    // DuckLake 标准表名（不同版本可能不同）
    for query in &[
        "SELECT name FROM duckdb_table_entries WHERE name NOT LIKE 'duckdb_%'",
        "SELECT table_name FROM duckdb_tables",
        "SELECT name FROM tables WHERE name NOT LIKE 'duckdb_%'",
    ] {
        if let Ok(mut stmt) = conn.prepare(query) {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                let names: Vec<String> = rows.filter_map(|r| r.ok()).collect();
                if !names.is_empty() {
                    // 同时查视图
                    let mut all = names;
                    if let Ok(mut vstmt) = conn.prepare(
                        "SELECT name FROM duckdb_view_entries WHERE name NOT LIKE 'duckdb_%'"
                    ) {
                        if let Ok(vrows) = vstmt.query_map([], |r| r.get::<_, String>(0)) {
                            all.extend(vrows.filter_map(|r| r.ok()));
                        }
                    }
                    return Ok(all);
                }
            }
        }
    }

    Ok(Vec::new())
}
