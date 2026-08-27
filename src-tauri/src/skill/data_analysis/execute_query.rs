use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{
    emit_tool_call, emit_tool_result, emit_tool_result_with_meta, next_tool_id,
};
use super::super::super::duckdb::{execute, QUERY_HARD_TIMEOUT_SECS};
use super::super::super::duckdb::attach::workspace_attach_alias;
use super::super::super::model::SqlResult;
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct ExecuteQueryArgs {
    sql: String,
}

pub struct ExecuteQueryTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

impl Tool for ExecuteQueryTool {
    const NAME: &'static str = "execute_query";
    type Error = ToolError;
    type Args = ExecuteQueryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "execute_query".to_string(),
            description: "执行只读的 SQL 查询并返回结果。禁止 DROP/ALTER/UPDATE/DELETE/INSERT/TRUNCATE/ATTACH/DETACH；结果超过 50 行自动截断，全量统计请先用聚合函数算，再按需下钻明细。查询外表时必须用 postgres_query 下推聚合（写法见系统提示第三步），不要查注册的视图——那会拉全表到本地。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "要执行的 SQL 查询语句" }
                },
                "required": ["sql"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let sql = args.sql.trim();
        if let Some(kw) = super::sql_forbidden_keyword(sql) {
            return Err(ToolError(format!("出于安全考虑，禁止执行包含 {} 操作的 SQL 语句。", kw)));
        }

        let call_id = next_tool_id("exec");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "execute_query",
            json!({ "sql": sql }),
        );

        let start = std::time::Instant::now();

        let wsc = match self.app_state.ensure_workspace_conn(&self.ws.path).await {
            Ok(w) => w,
            Err(msg) => {
                let full = format!("DuckDB 引擎未就绪: {msg}");
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    full.clone(), Some(sql.to_string()), None, Some(0), None,
                );
                return Err(ToolError(full));
            }
        };
        let conn = wsc.conn.clone();
        let ih = wsc.interrupt_handle.lock().ok().map(|g| g.clone());

        let sql_string = sql.to_string();

        // 别名归一化兜底：LLM 常把 postgres_query 的 catalog 别名写成原始连接名
        // （如 `demo` 而非 `db_demo`），导致 binder 报
        // "Failed to find attached database"。执行前据已注册连接自动改写。
        // 多数查询不含 postgres_query，先快速过滤避免无谓的 DB 加载。
        let sql_string = if sql.to_ascii_lowercase().contains("postgres_query") {
            let ws_path = self.ws.path.clone();
            let raw = sql_string.clone();
            match tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
                let conns = crate::db::list_workspace_db_connections(&ws_path).map_err(|e| e.to_string())?;
                let norm = normalize_pg_query_aliases(&raw, &conns);
                if norm != raw { Ok(Some(norm)) } else { Ok(None) }
            }).await {
                Ok(Ok(Some(norm))) => {
                    tracing::info!(category = "agent", "postgres_query 别名已自动归一化（原始连接名 → db_ 别名）");
                    norm
                }
                // 未变化或加载失败，都用原 SQL（不阻断查询）。
                _ => sql_string,
            }
        } else {
            sql_string
        };

        let hard_secs = QUERY_HARD_TIMEOUT_SECS;

        // 实际落 DuckDB 的语句（别名归一化 + 行数包装）——日志与 transcript meta
        // 都记录它，与模型原文 sql 对照即可复现"当时到底查了什么"。
        let sql_executed = execute::wrap_query(&sql_string, Some(50));

        // 执行前检查：解析 SQL 里的表名，查 table_registry access_mode。
        // pushdown 模式的表不能通过视图查询（会拉全表），必须用 postgres_query 下推。
        let ws_path = self.ws.path.clone();
        let sql_for_check = sql_string.clone();
        let pushdown_violation = {
            let ws = ws_path.clone();
            tokio::task::spawn_blocking(move || -> Option<(String, String, String, String)> {
                // 提取 SQL 里的所有 FROM 表名
                let tables = extract_table_names(&sql_for_check);
                for t in &tables {
                    if let Ok(Some(entry)) = crate::db::get_table_registry_by_local_name(&ws, t) {
                        if entry.access_mode == "pushdown" {
                            return Some((t.clone(), entry.connection_name, entry.remote_schema, entry.remote_table));
                        }
                    }
                }
                None
            }).await
        };

        // pushdown 检查
        if let Ok(Some((table_name, conn_name, schema, remote_table))) = &pushdown_violation {
            let msg = format!(
                "表 `{}` 是 pushdown 模式（Hologres 外部库外表），不能通过视图查询（会拉全表到本地，非常慢）。\n\
                 请用 postgres_query 下推查询：\n\
                 SELECT * FROM postgres_query('db_{}', 'SELECT ... FROM \"{}\".\"{}\" WHERE ... GROUP BY ...')",
                table_name, conn_name, schema, remote_table
            );
            emit_tool_result(
                &self.window, &self.task_id, &call_id, "error",
                msg.clone(), Some(sql_string.clone()), None, Some(0), None,
            );
            return Err(ToolError(msg));
        }

        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<SqlResult, String> {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(50)).map_err(|e| {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                if lower.contains("permission denied") || lower.contains("access denied") {
                    format!("查询失败：当前用户没有查询权限。错误: {msg}\n建议：检查数据源连接的用户是否有该表的查询权限，或换一张表。")
                } else if (lower.contains("does not exist") || lower.contains("not found")) && lower.contains("column") {
                    // 远端列名写错（如 pay_time 应为 payment_time）：不要包装成
                    // "表或视图不存在"，否则会诱导 agent 重新注册已存在的表。
                    format!("查询失败：SQL 中引用的列不存在。错误: {msg}\n建议：用 describe_table 查看该表的真实列名，修正 SQL 后重试。")
                } else if lower.contains("does not exist") || lower.contains("not found") {
                    format!("查询失败：表或视图不存在。错误: {msg}\n建议：用 list_tables 确认表名是否正确，或用 list_remote_tables 重新探查。")
                } else {
                    format!("SQL 执行出错: {msg}")
                }
            })
        });
        let query_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(ToolError)),
                Err(_) => {
                    if let Some(ih) = ih {
                        ih.interrupt();
                    }
                    Err(ToolError(format!("SQL 执行已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(ToolError))
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match query_res {
            Ok(res) => {
                let n = res.rows.len();
                let summary = format!("查询成功，返回 {} 行（{} 列）", n, res.columns.len());
                // 给 LLM 的紧凑文本（避免 50 行灌满上下文）；
                // 完整结构化 SqlResult 通过 payload 发给前端。
                let mut out = String::new();
                out.push_str(&format!("查询成功，返回 {} 行。列: {}\n", n, res.columns.join(", ")));
                for (i, row) in res.rows.iter().enumerate() {
                    let row_str: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                    out.push_str(&format!("行 #{}: {}\n", i + 1, row_str.join(" | ")));
                }
                if res.truncated {
                    out.push_str("(结果已截断，仅返回前 50 行)\n");
                }
                let payload = serde_json::to_value(&res).ok();
                // 结果指纹（首行值，截断控体积）：复盘对账用——「同窗不同数」
                // 这类引擎级异常，事后要能从 logs 表回放当时工具返回了什么
                // （2026-08-27 排查 P0-B 时因日志只有行数摘要而无从对账）。
                let first_row: Vec<String> = res
                    .rows
                    .first()
                    .map(|r| {
                        r.iter()
                            .take(16)
                            .map(|v| {
                                let s = v.to_string();
                                if s.chars().count() > 64 { s.chars().take(64).collect() } else { s }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let meta = serde_json::json!({
                    "sql": sql_executed,
                    "sqlOriginal": sql,
                    "rows": n,
                    "truncated": res.truncated,
                    "firstRow": first_row,
                    "errorClass": serde_json::Value::Null,
                });
                let ws_log = self.ws.path.clone();
                let task_log = self.task_id.clone();
                tracing::info!(
                    category = "sql",
                    workspace = ws_log.as_str(),
                    task_id = task_log.as_str(),
                    detail = ?meta,
                    "execute_query 成功，返回 {} 行（{} ms）",
                    n, elapsed,
                );
                emit_tool_result_with_meta(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, Some(sql.to_string()), payload, Some(elapsed), None, Some(meta),
                );
                Ok(out)
            }
            Err(err) => {
                // 查询失败时更新 table_registry 的表状态（status 真源在 SQLite）。
                let ws_path = self.ws.path.clone();
                let err_msg = err.0.clone();
                let err_class = super::classify_sql_error_class(&err_msg);
                let sql_clone = sql.to_string();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Some(table_name) = extract_table_from_sql(&sql_clone) {
                        let (status, reason) = classify_query_error(&err_msg);
                        let _ = crate::db::update_table_registry_status(&ws_path, &table_name, &status, Some(&reason));
                    }
                }).await;
                let meta = serde_json::json!({
                    "sql": sql_executed,
                    "sqlOriginal": sql,
                    "rows": 0,
                    "truncated": false,
                    "errorClass": err_class,
                });
                let ws_log = self.ws.path.clone();
                let task_log = self.task_id.clone();
                tracing::error!(
                    category = "sql",
                    workspace = ws_log.as_str(),
                    task_id = task_log.as_str(),
                    detail = ?meta,
                    "execute_query 失败({}): {}",
                    err_class, err.0,
                );
                emit_tool_result_with_meta(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), Some(sql.to_string()), None, Some(elapsed), None, Some(meta),
                );
                Err(err)
            }
        }
    }
}

/// 从 SQL 中提取第一个 FROM/JOIN 表名（错误路径更新 table_registry 状态用）。
/// 复用 extract_table_names 的词边界解析——此前独立的子串匹配在多行 SQL
/// 上取错 token，状态更新从未生效过。
fn extract_table_from_sql(sql: &str) -> Option<String> {
    extract_table_names(sql).into_iter().next()
}

/// 根据查询错误判定可用性等级。
fn classify_query_error(err: &str) -> (String, String) {
    let lower = err.to_lowercase();
    if lower.contains("storage tier") || lower.contains("lower meta") || lower.contains("odps") {
        ("unavailable_permanent".to_string(), "MaxCompute 非标准存储，Hologres 不支持访问".to_string())
    } else if lower.contains("unsupported type") {
        ("unavailable_permanent".to_string(), "列类型不兼容".to_string())
    } else if lower.contains("permission denied") || lower.contains("access denied") {
        ("unavailable_temporary".to_string(), "权限不足".to_string())
    } else if lower.contains("timeout") || lower.contains("connection") {
        ("unavailable_temporary".to_string(), "连接超时".to_string())
    } else {
        // 未知错误不改状态（可能是 SQL 语法错误，不是表不可用）。
        ("available".to_string(), String::new())
    }
}

/// 从 SQL 中提取 FROM / JOIN 后面的表名（供 pushdown 守卫比对注册表）。
///
/// 关键词判定必须是**词边界匹配**（前一个字符不是标识符字符、后一个字符
/// 不是字母数字/下划线），而不是字面 `" FROM"`（空格）子串匹配——LLM 写的
/// SQL 几乎都是多行格式化的，FROM 前面是换行符，空格匹配会让守卫整个失效
/// （2026-08-27 复盘实证：一单分析里 15 次 24–45s 的全表拉取慢查询全部
/// 绕过守卫，根因即此）。支持 `FROM a, b` 逗号表清单；跳过子查询的
/// SELECT/VALUES 等关键词。基于 char 数组逐位比较，避开 `to_uppercase`
/// 可能改变字节长度导致索引错位的问题。
fn extract_table_names(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut result = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if table_kw_at(&chars, i, "FROM") || table_kw_at(&chars, i, "JOIN") {
            i += 4; // FROM / JOIN 等长
            loop {
                let (name, next) = read_table_token(&chars, i);
                i = next;
                if !name.is_empty() && !is_sql_keyword(&name) {
                    result.push(name);
                }
                // `FROM a, b` 逗号表清单：下一个非空字符是 ',' 且其后像表名才续读。
                // 续读必须从逗号之后（j+1）开始——read_table_token 遇 ',' 即停，
                // 若从 j 本身读会原地返回空 token、i 永不前进（死循环，测试已拦）。
                let mut j = i;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ',' {
                    let (peek, _) = read_table_token(&chars, j + 1);
                    if !peek.is_empty() && !is_sql_keyword(&peek) {
                        i = j + 1;
                        continue;
                    }
                }
                break;
            }
        } else {
            i += 1;
        }
    }
    result
}

/// chars[i..] 是否等于关键词（ASCII 大小写不敏感），且前后都是词边界。
fn table_kw_at(chars: &[char], i: usize, kw: &str) -> bool {
    let is_ident_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let kc: Vec<char> = kw.chars().collect();
    i + kc.len() <= chars.len()
        && (0..kc.len()).all(|k| chars[i + k].eq_ignore_ascii_case(&kc[k]))
        && (i == 0 || !is_ident_char(chars[i - 1]))
        && (i + kc.len() == chars.len() || !is_ident_char(chars[i + kc.len()]))
}

/// 从 chars[i..] 读一个表名 token（引号内允许空白），跳过前导空白与开括号。
fn read_table_token(chars: &[char], mut i: usize) -> (String, usize) {
    while i < chars.len() && (chars[i].is_whitespace() || chars[i] == '(') {
        i += 1;
    }
    let start = i;
    let mut in_quotes = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            in_quotes = !in_quotes;
            i += 1;
            continue;
        }
        if !in_quotes && (c.is_whitespace() || c == ',' || c == ')' || c == ';' || c == '(') {
            break;
        }
        i += 1;
    }
    (chars[start..i].iter().collect::<String>().trim_matches('"').to_string(), i)
}

/// 子查询/字面量开头词不是表名。
fn is_sql_keyword(name: &str) -> bool {
    matches!(
        name.to_uppercase().as_str(),
        "SELECT" | "VALUES" | "LATERAL" | "UNNEST" | "WHERE" | "ON" | "USING" | "AS"
    ) || name.starts_with('\'')
}

/// 归一化 SQL 里 `postgres_query` 第一个参数（catalog 别名）。
///
/// LLM 常把 catalog 别名写成原始连接名（如 `demo` 而非 `db_demo`），
/// 导致 DuckDB binder 报 "Failed to find attached database"。这里扫描每个
/// `postgres_query('xxx', ...)`，若 `xxx` 是已注册连接的原始名，就改写成
/// `db_<xxx>`，在执行前兜底纠偏。连接名恒为 ASCII，切片边界均为字符边界。
fn normalize_pg_query_aliases(sql: &str, conns: &[crate::model::DataSourceConfig]) -> String {
    // 原始连接名 -> catalog 别名（别名恒为 db_<safe>，与原始名不同才需改写）。
    let map: std::collections::HashMap<&str, String> = conns
        .iter()
        .map(|c| (c.name.as_str(), workspace_attach_alias(&c.name)))
        .filter(|(raw, alias)| *raw != alias.as_str())
        .collect();
    if map.is_empty() {
        return sql.to_string();
    }

    let lower = sql.to_ascii_lowercase(); // 逐字节小写，字节偏移与原串对齐
    let needle = "postgres_query";
    let nlen = needle.len();
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 32);
    let mut cursor = 0usize;

    while let Some(rel) = lower[cursor..].find(needle) {
        let hit = cursor + rel; // 'postgres_query' 起始字节
        let after_kw = hit + nlen;

        // 跳过空白找 '('；否则当作普通词继续扫。
        let mut p = after_kw;
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if p >= bytes.len() || bytes[p] != b'(' {
            out.push_str(&sql[cursor..after_kw]);
            cursor = after_kw;
            continue;
        }
        let open_paren = p;
        // 跳过空白找开引号 '。
        let mut q = open_paren + 1;
        while q < bytes.len() && bytes[q].is_ascii_whitespace() {
            q += 1;
        }
        if q >= bytes.len() || bytes[q] != b'\'' {
            // 第一个参数不是单引号字符串字面量，原样输出到 '(' 后并继续扫。
            out.push_str(&sql[cursor..=open_paren]);
            cursor = open_paren + 1;
            continue;
        }
        let quote_open = q;
        // 读到下一个单引号（连接名为 ASCII，不含 '，无需处理 '' 转义）。
        let mut r = quote_open + 1;
        while r < bytes.len() && bytes[r] != b'\'' {
            r += 1;
        }
        if r >= bytes.len() {
            // 未闭合的引号，原样输出剩余。
            out.push_str(&sql[cursor..]);
            cursor = bytes.len();
            break;
        }
        let quote_close = r;
        let name = &sql[quote_open + 1..quote_close];
        if let Some(alias) = map.get(name) {
            out.push_str(&sql[cursor..quote_open]); // 含 'postgres_query(...' 到开引号前
            out.push('\'');
            out.push_str(alias);
            out.push('\'');
            cursor = quote_close + 1;
        } else {
            // 不是已知原始名（可能已是别名或无关内容），原样输出整个字面量。
            out.push_str(&sql[cursor..=quote_close]);
            cursor = quote_close + 1;
        }
    }
    if cursor < bytes.len() {
        out.push_str(&sql[cursor..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DataSourceConfig;

    fn conn(name: &str) -> DataSourceConfig {
        DataSourceConfig {
            id: name.into(),
            name: name.into(),
            db_type: "postgres".into(),
            host: "h".into(),
            port: 5432,
            database_name: "db".into(),
            username: "u".into(),
            password: "p".into(),
            ssl_mode: "disable".into(),
            created_at: 0,
            db_product: "hologres".into(),
            db_mode: "standard".into(),
        }
    }

    #[test]
    fn rewrites_raw_name_to_alias() {
        let sql = "SELECT * FROM postgres_query('demo', 'SELECT 1')";
        let out = normalize_pg_query_aliases(sql, &[conn("demo")]);
        assert_eq!(out, "SELECT * FROM postgres_query('db_demo', 'SELECT 1')");
    }

    #[test]
    fn leaves_correct_alias_untouched() {
        let sql = "SELECT * FROM postgres_query('db_demo', 'SELECT 1')";
        let out = normalize_pg_query_aliases(sql, &[conn("demo")]);
        assert_eq!(out, sql);
    }

    #[test]
    fn no_postgres_query_untouched() {
        let sql = "SELECT * FROM v_orders WHERE ds='20260811'";
        let out = normalize_pg_query_aliases(sql, &[conn("demo")]);
        assert_eq!(out, sql);
    }

    #[test]
    fn handles_whitespace_and_case() {
        let sql = "select * from POSTGRES_QUERY ( 'demo' , 'SELECT 1' )";
        let out = normalize_pg_query_aliases(sql, &[conn("demo")]);
        assert_eq!(out, "select * from POSTGRES_QUERY ( 'db_demo' , 'SELECT 1' )");
    }

    // ------------------------------------------------------------------
    // extract_table_names：词边界表名提取（pushdown 守卫的耳目）。
    // 2026-08-27 复盘：字面 " FROM"（空格）匹配在多行 SQL 上全数漏检，
    // 守卫形同虚设。以下用例按真实翻车 SQL 的形状回归。
    // ------------------------------------------------------------------

    #[test]
    fn extracts_table_after_newline_from() {
        // 复盘实录形状：FROM 前是换行符——旧实现完全匹配不到。
        let sql = "SELECT scrm_dept_lv2_name AS dept2, COUNT(*) AS ord_cnt, ROUND(SUM(real_payment),0) AS rev\nFROM v_sale_dept_order_detail\nWHERE ds='20260811'";
        assert_eq!(extract_table_names(sql), vec!["v_sale_dept_order_detail"]);
    }

    #[test]
    fn extracts_table_after_indented_from_and_join() {
        let sql = "SELECT a.x\n  FROM v_a\n  JOIN v_b ON a.id = b.id\n WHERE a.y = 1";
        assert_eq!(extract_table_names(sql), vec!["v_a", "v_b"]);
    }

    #[test]
    fn single_line_space_from_still_works() {
        assert_eq!(extract_table_names("SELECT * FROM v_orders"), vec!["v_orders"]);
    }

    #[test]
    fn skips_subquery_select_and_finds_inner_table() {
        let sql = "SELECT * FROM (SELECT id FROM v_inner) t JOIN v_outer USING (id)";
        assert_eq!(extract_table_names(sql), vec!["v_inner", "v_outer"]);
    }

    #[test]
    fn ignores_keywords_inside_identifiers() {
        // is_from_x 里的 "from" 不是关键词；列名 from_city 同理。
        let sql = "SELECT is_from_x FROM t1 WHERE col = 'from'";
        assert_eq!(extract_table_names(sql), vec!["t1"]);
    }

    #[test]
    fn quoted_and_quoted_schema_names() {
        assert_eq!(
            extract_table_names("SELECT 1 FROM \"my table\", v2"),
            vec!["my table", "v2"]
        );
    }

    #[test]
    fn error_path_first_table_extraction() {
        let sql = "SELECT COUNT(*)\nFROM v_lead_supply_mf\nWHERE ds IN ('2026-07-01')";
        assert_eq!(extract_table_from_sql(sql), Some("v_lead_supply_mf".to_string()));
    }

    // ------------------------------------------------------------------
    // sql_forbidden_keyword：词边界禁词守卫（is_deleted 不再误伤）。
    // ------------------------------------------------------------------

    #[test]
    fn is_deleted_column_not_flagged() {
        // 复盘实录：`target_is_deleted = false` 被子串匹配误拦成 DELETE。
        assert!(super::super::sql_forbidden_keyword(
            "SELECT metric FROM t WHERE target_is_deleted = false AND is_deleted = false"
        )
        .is_none());
    }

    #[test]
    fn real_delete_statement_flagged() {
        assert_eq!(
            super::super::sql_forbidden_keyword("delete FROM orders WHERE 1=1"),
            Some("DELETE")
        );
        assert_eq!(
            super::super::sql_forbidden_keyword("SELECT 1; DROP VIEW v_x"),
            Some("DROP")
        );
    }

    #[test]
    fn keyword_in_string_literal_not_flagged_but_unclosed_quote_falls_back() {
        // 字面量里的 delete 不是指令。
        assert!(super::super::sql_forbidden_keyword(
            "SELECT * FROM t WHERE note = 'please do not delete anything'"
        )
        .is_none());
        // 引号未闭合 → 保守回退子串匹配（宁可误拦）。
        assert_eq!(
            super::super::sql_forbidden_keyword("SELECT * FROM t WHERE note = 'delete"),
            Some("DELETE")
        );
    }

    #[test]
    fn keyword_adjacent_punctuation_flagged() {
        assert_eq!(
            super::super::sql_forbidden_keyword("TRUNCATE(table_x)"),
            Some("TRUNCATE")
        );
        assert_eq!(
            super::super::sql_forbidden_keyword("ALTER TABLE t ADD COLUMN c INT"),
            Some("ALTER")
        );
    }

    #[test]
    fn preserves_inner_query_with_chinese_and_quotes() {
        let sql = "SELECT * FROM postgres_query('demo', 'SELECT * FROM \"default\".\"t\" WHERE report_module = ''5_示例部门'' ORDER BY 1')";
        let out = normalize_pg_query_aliases(sql, &[conn("demo")]);
        assert_eq!(
            out,
            "SELECT * FROM postgres_query('db_demo', 'SELECT * FROM \"default\".\"t\" WHERE report_module = ''5_示例部门'' ORDER BY 1')"
        );
    }

    #[test]
    fn rewrites_multiple_occurrences() {
        let sql = "SELECT * FROM postgres_query('demo', 'SELECT 1') JOIN postgres_query('demo', 'SELECT 2')";
        let out = normalize_pg_query_aliases(sql, &[conn("demo")]);
        assert_eq!(
            out,
            "SELECT * FROM postgres_query('db_demo', 'SELECT 1') JOIN postgres_query('db_demo', 'SELECT 2')"
        );
    }

    #[test]
    fn ignores_unknown_name() {
        let sql = "SELECT * FROM postgres_query('other', 'SELECT 1')";
        let out = normalize_pg_query_aliases(sql, &[conn("demo")]);
        assert_eq!(out, sql);
    }
}
