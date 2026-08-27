use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::super::duckdb::attach::workspace_attach_alias;
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ListRemoteTablesArgs {
    connection_name: String,
    /// 可选关键字子串：只返回表名（schema.table）或注释包含它的表。
    #[serde(default)]
    filter: Option<String>,
}

/// 一张远程表的名级元数据。注释/行数仅在 postgres 富查询成功时有值。
#[derive(Debug, Clone)]
pub struct RemoteTable {
    pub schema: String,
    pub name: String,
    pub comment: String,
    /// pg_class.reltuples 估算；未分析过的表为 -1/0，展示时按无值处理。
    pub est_rows: i64,
}

/// filter 是否命中（schema.table + 注释，大小写不敏感子串；空 filter 恒命中）。
fn matches_filter(t: &RemoteTable, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    if f.is_empty() {
        return true;
    }
    format!("{}.{} {}", t.schema, t.name, t.comment)
        .to_lowercase()
        .contains(&f)
}

/// 渲染一行表条目：`- schema.table — 注释 (≈1.2M 行)`，注释/行数缺省时退化。
fn format_table_line(t: &RemoteTable) -> String {
    let mut line = format!("- {}.{}", t.schema, t.name);
    let comment = t.comment.trim();
    if !comment.is_empty() {
        line.push_str(&format!(" — {comment}"));
    }
    if t.est_rows > 0 {
        line.push_str(&format!(" (≈{} 行)", fmt_rows(t.est_rows)));
    }
    line
}

/// 行数缩写：≥1M → `1.2M`，≥1k → `12.3k`，否则原样。
fn fmt_rows(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub struct ListRemoteTablesTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for ListRemoteTablesTool {
    const NAME: &'static str = "list_remote_tables";
    type Error = ToolError;
    type Args = ListRemoteTablesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_remote_tables".to_string(),
            description: "探查指定数据源连接下有哪些表和视图。返回 schema.table 格式的表名；postgres 类型还会带表注释和行数估算。数据源表很多（几百上千张）时传 filter 关键词缩小范围。用于从数据源中发现与用户分析目标相关的表。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "connection_name": { "type": "string", "description": "数据源名称（如 myshop）" },
                    "filter": { "type": "string", "description": "可选。不区分大小写的子串，只返回表名或注释包含它的表（如 order、销量）。表多时先用它缩小范围再选。" }
                },
                "required": ["connection_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("rt");
        let conn_name = args.connection_name.trim();
        let filter = args.filter.as_deref().unwrap_or("").trim().to_string();
        let mut call_args = json!({ "connection_name": conn_name });
        if !filter.is_empty() {
            call_args["filter"] = json!(filter);
        }
        emit_tool_call(&self.window, &self.task_id, &call_id, "list_remote_tables", call_args);

        let start = std::time::Instant::now();

        // 从 SQLite 查连接信息。
        let ws_path = self.ws.path.clone();
        let conn_record = {
            let ws_path = ws_path.clone();
            let name = conn_name.to_string();
            tokio::task::spawn_blocking(move || {
                crate::db::get_workspace_db_connection_by_name(&ws_path, &name)
            }).await
            .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
            .map_err(|e| ToolError(e))?
        };

        let conn_record = match conn_record {
            Some(c) => c,
            None => {
                let msg = format!("数据源 {} 不存在或未启用。", conn_name);
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", msg.clone(), None, None, Some(0), None);
                return Err(ToolError(msg));
            }
        };

        let wsc = match self.app_state.ensure_workspace_conn(&self.ws.path).await {
            Ok(w) => w,
            Err(msg) => {
                let full = format!("DuckDB 引擎未就绪: {msg}");
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", full.clone(), None, None, Some(0), None);
                return Err(ToolError(full));
            }
        };
        let duckdb_conn = wsc.conn.clone();

        let db_type = conn_record.db_type.clone();
        let catalog = workspace_attach_alias(&conn_record.name);
        let tables_res = tokio::task::spawn_blocking(move || -> Result<Vec<RemoteTable>, String> {
            let guard = duckdb_conn.blocking_lock();

            if db_type == "postgres" {
                // postgres 类型：用 postgres_query 下推查 pg_catalog，
                // 传 catalog 别名（db_xxx）而非连接串。
                // 查 pg_class + pg_namespace 能列出所有表（含外表 relkind='f'），
                // information_schema 在 Hologres 上可能不包含外表。
                //
                // 先尝试富查询（表级注释 objsubid=0 + reltuples 行数估算；
                // 子查询取注释避免一表多列注释产生重复行）。部分兼容 PG 协议的
                // 库系统目录有差异，富查询失败时回退纯名单，注释/行数留空。
                let rich_inner = "SELECT n.nspname AS table_schema, c.relname AS table_name, \
                    COALESCE((SELECT d.description FROM pg_catalog.pg_description d \
                    WHERE d.objoid = c.oid AND d.objsubid = 0 LIMIT 1), '') AS table_comment, \
                    c.reltuples::bigint AS est_rows \
                    FROM pg_catalog.pg_class c \
                    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                    WHERE c.relkind IN (''r'', ''v'', ''m'', ''f'', ''p'') \
                    AND n.nspname NOT IN (''pg_catalog'', ''information_schema'', ''pg_toast'') \
                    ORDER BY n.nspname, c.relname";
                let rich_sql = format!(
                    "SELECT * FROM postgres_query('{}', '{}')",
                    catalog, rich_inner
                );
                let rich: Option<Vec<RemoteTable>> = (|| {
                    let mut stmt = guard.prepare(&rich_sql).ok()?;
                    let rows = stmt
                        .query_map([], |r| {
                            Ok(RemoteTable {
                                schema: r.get(0)?,
                                name: r.get(1)?,
                                comment: r.get(2)?,
                                est_rows: r.get(3)?,
                            })
                        })
                        .ok()?;
                    rows.collect::<Result<Vec<_>, _>>().ok()
                })();
                if let Some(list) = rich {
                    return Ok(list);
                }
                // 回退：纯名单（两列），与历史行为一致。
                let inner_sql = "SELECT n.nspname AS table_schema, c.relname AS table_name \
                    FROM pg_catalog.pg_class c \
                    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                    WHERE c.relkind IN (''r'', ''v'', ''m'', ''f'', ''p'') \
                    AND n.nspname NOT IN (''pg_catalog'', ''information_schema'', ''pg_toast'') \
                    ORDER BY n.nspname, c.relname";
                let sql = format!(
                    "SELECT * FROM postgres_query('{}', '{}')",
                    catalog, inner_sql
                );
                let mut stmt = guard.prepare(&sql).map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(RemoteTable {
                            schema: r.get(0)?,
                            name: r.get(1)?,
                            comment: String::new(),
                            est_rows: 0,
                        })
                    })
                    .map_err(|e| e.to_string())?;
                let mut list = Vec::new();
                for r in rows {
                    list.push(r.map_err(|e| e.to_string())?);
                }
                Ok(list)
            } else {
                // mysql/sqlite：走 DuckDB catalog（这些类型的元数据扫描不报错）。
                let catalog = workspace_attach_alias(&conn_record.name);
                guard.execute_batch(&format!("USE {}", catalog)).map_err(|e| e.to_string())?;
                let mut stmt = guard.prepare("SHOW TABLES").map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |r| {
                        let name: String = r.get(0)?;
                        let parts: Vec<&str> = name.splitn(2, '.').collect();
                        let schema = if parts.len() == 2 { parts[0].to_string() } else { "main".to_string() };
                        let table = if parts.len() == 2 { parts[1].to_string() } else { parts[0].to_string() };
                        Ok(RemoteTable { schema, name: table, comment: String::new(), est_rows: 0 })
                    })
                    .map_err(|e| e.to_string())?;
                let mut list = Vec::new();
                for r in rows {
                    list.push(r.map_err(|e| e.to_string())?);
                }
                // 恢复默认 catalog 为 lake（不能恢复成 memory，否则后续未限定的
                // CREATE VIEW 会落进 in-memory 库，重启即丢——而 table_registry
                // 仍记录它存在，造成"索引有、沉淀无"的不一致。
                let _ = guard.execute_batch("USE lake");
                Ok(list)
            }
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        .map_err(ToolError);

        let elapsed = start.elapsed().as_millis() as u64;
        match tables_res {
            Ok(tables) => {
                let total = tables.len();
                let shown: Vec<&RemoteTable> = tables
                    .iter()
                    .filter(|t| matches_filter(t, &filter))
                    .collect();
                let (summary, out) = if shown.is_empty() {
                    if filter.is_empty() {
                        (
                            format!("数据源 {} 中没有找到任何表。", conn_name),
                            format!("数据源 {} 中没有找到任何表。", conn_name),
                        )
                    } else {
                        (
                            format!("数据源 {} 中没有匹配「{filter}」的表（共 {total} 张）。", conn_name),
                            format!("数据源 {} 中没有匹配 filter「{filter}」的表（共 {total} 张）。可以去掉 filter 用全量名单，或换一个关键词。", conn_name),
                        )
                    }
                } else if filter.is_empty() {
                    let body = shown.iter().map(|t| format_table_line(t)).collect::<Vec<_>>().join("\n");
                    (
                        format!("数据源 {} 中有 {} 张表", conn_name, shown.len()),
                        format!("数据源 {} 中的表：\n{}", conn_name, body),
                    )
                } else {
                    let body = shown.iter().map(|t| format_table_line(t)).collect::<Vec<_>>().join("\n");
                    (
                        format!("数据源 {} 匹配「{filter}」：{} / {} 张表", conn_name, shown.len(), total),
                        format!("数据源 {} 中匹配 filter「{filter}」的表（{} / 共 {} 张）：\n{}", conn_name, shown.len(), total, body),
                    )
                };
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, None, None, Some(elapsed), Some(out.clone()));
                Ok(out)
            }
            Err(err) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", err.0.clone(), None, None, Some(elapsed), None);
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(schema: &str, name: &str, comment: &str, rows: i64) -> RemoteTable {
        RemoteTable {
            schema: schema.into(),
            name: name.into(),
            comment: comment.into(),
            est_rows: rows,
        }
    }

    #[test]
    fn fmt_rows_units() {
        assert_eq!(fmt_rows(999), "999");
        assert_eq!(fmt_rows(1_000), "1.0k");
        assert_eq!(fmt_rows(12_345), "12.3k");
        assert_eq!(fmt_rows(1_234_567), "1.2M");
        // 未分析过的表（-1/0）按无值处理，由调用方过滤，这里只保证不 panic。
        assert_eq!(fmt_rows(0), "0");
    }

    #[test]
    fn filter_matches_name_or_comment_case_insensitive() {
        let tables = vec![
            rt("default", "orders", "订单主表", 1_000),
            rt("ods", "orders_raw", "", 0),
            rt("dim", "shop", "门店维度表", 50),
        ];
        let f = |q: &str| {
            tables
                .iter()
                .filter(|t| matches_filter(t, q))
                .map(|t| format!("{}.{}", t.schema, t.name))
                .collect::<Vec<_>>()
        };
        // 命中表名
        assert_eq!(f("ORDER"), vec!["default.orders", "ods.orders_raw"]);
        // 命中注释（中文）
        assert_eq!(f("门店"), vec!["dim.shop"]);
        // 空 filter 全量
        assert_eq!(f("").len(), 3);
        // 无命中
        assert!(f("nope").is_empty());
    }

    #[test]
    fn format_line_degrades_gracefully() {
        assert_eq!(
            format_table_line(&rt("default", "orders", "订单主表，含支付状态", 1_234_567)),
            "- default.orders — 订单主表，含支付状态 (≈1.2M 行)"
        );
        assert_eq!(format_table_line(&rt("ods", "t", "", 0)), "- ods.t");
        // 未分析的行数估算（-1）不显示
        assert_eq!(format_table_line(&rt("ods", "t", "", -1)), "- ods.t");
        assert_eq!(format_table_line(&rt("ods", "t", "无行数注释", 0)), "- ods.t — 无行数注释");
    }
}
