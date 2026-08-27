//! 数据分析场景工具——查询闭环。
//!
//! P1 含 4 个只读查询工具（execute_query / list_tables / describe_table /
//! sample_data）。DDL 加工工具（create_table / create_view / drop_object）
//! 和 OKF 知识库工具留待后续阶段。

pub mod create_view;
pub mod delete_okf_knowledge;
pub mod describe_table;
pub mod drop_object;
pub mod execute_query;
pub mod list_connections;
pub mod list_okf_knowledge;
pub mod list_remote_tables;
pub mod list_tables;
pub mod load_okf_knowledge;
pub mod register_table;
pub mod rename_okf_knowledge;
pub mod render_chart;
pub mod read_okf_metadata;
pub mod sample_data;
pub mod search_okf_knowledge;
pub mod update_okf_metadata;
pub mod write_okf_knowledge;

/// 剥掉 postgres 扩展 COPY binary 协议的错误包装，暴露真实根因。
///
/// postgres_query 取数走 `COPY (…) TO STDOUT (FORMAT binary)`，出错时 DuckDB
/// 报 `Failed to prepare COPY "<整段内层SQL>";": <真实错误>`——根因（权限/
/// 表不存在/类型转换失败）被一大段 SQL 前缀淹没，模型只能靠猜（2026-08-27
/// 复盘：6 次此类报错全部需要模型反复试错解读）。剥壳规则：截取 COPY 语句
/// 结束标记 `;": ` 之后的文本；`Failed to fetch header for COPY` 同理；
/// 都不匹配则原样返回。
pub(crate) fn decapsulate_copy_error(err: &str) -> String {
    let strip_lead = |s: &str| -> String {
        let mut s = s.trim_start();
        // COPY 错误尾格式有 `;": ERROR:  xxx` 与 `: ERROR:  xxx` 两种，
        // 剥掉可选的冒号后再剥 ERROR: 标签。
        if let Some(r) = s.strip_prefix(':') {
            s = r.trim_start();
        }
        s.strip_prefix("ERROR:")
            .map(|r| r.trim_start().to_string())
            .unwrap_or_else(|| s.to_string())
    };
    if let Some(pos) = err.rfind(";\": ") {
        // 确认这是 COPY 包装（防误伤恰好含该序列的普通错误）
        if err.contains("Failed to prepare COPY \"") {
            return strip_lead(&err[pos + 4..]);
        }
    }
    if let Some(rest) = err.split_once("Failed to fetch header for COPY") {
        return strip_lead(rest.1);
    }
    err.to_string()
}

/// 归并 SQL 执行错误到 compact class，供 logs 表与 transcript meta 共用。
/// 值域固定，后续 agent 调优 / 历史排查可直接按此维度过滤。
pub(crate) fn classify_sql_error_class(err: &str) -> &'static str {
    let lower = err.to_lowercase();
    if lower.contains("permission denied")
        || lower.contains("access denied")
        || lower.contains("check permission")
    {
        "permission"
    } else if lower.contains("timeout") || lower.contains("interrupt") || lower.contains("已自动中断") {
        "timeout"
    } else if (lower.contains("does not exist") || lower.contains("not found"))
        && lower.contains("column")
    {
        "column_missing"
    } else if lower.contains("does not exist") || lower.contains("not found") {
        "table_missing"
    } else if lower.contains("invalid input syntax") || lower.contains("cast") {
        "cast_error"
    } else if lower.contains("syntax") || lower.contains("parser") || lower.contains("binder") {
        "syntax"
    } else {
        "other"
    }
}

/// SQL 只读守卫：检测禁用关键词（DROP/DELETE/…）是否以**独立词**出现。
///
/// 朴素 `contains` 子串匹配会把 `is_deleted` 这类合法列名误判成 DELETE
/// （2026-08-27 复盘实测：`target_is_deleted = false` 过滤条件被守卫拦下，
/// 模型被迫丢掉该条件）。这里先把单引号字面量的内容抹掉、再做词边界匹配
/// （前后不是字母数字/下划线）。若引号未闭合（可疑输入），退回保守的
/// 子串匹配——守卫宁可误拦，不可漏放。
pub(crate) fn sql_forbidden_keyword(sql: &str) -> Option<&'static str> {
    const KEYWORDS: [&str; 8] = [
        "DROP", "DELETE", "UPDATE", "INSERT", "ALTER", "TRUNCATE", "ATTACH", "DETACH",
    ];

    // 抹除 '...' 字面量内容（'' 转义经两次翻转自然回到串内），保留引号作边界。
    let mut blanked = String::with_capacity(sql.len());
    let mut in_string = false;
    for c in sql.chars() {
        if c == '\'' {
            in_string = !in_string;
            blanked.push('\'');
        } else if in_string {
            blanked.push(' ');
        } else {
            blanked.push(c);
        }
    }
    // 引号未闭合：无法可靠区分字面量与代码，退回朴素子串匹配（保守）。
    if in_string {
        let upper = sql.to_uppercase();
        return KEYWORDS.iter().find(|kw| upper.contains(*kw)).copied();
    }

    let chars: Vec<char> = blanked.chars().collect();
    let is_ident_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for kw in KEYWORDS {
        let kc: Vec<char> = kw.chars().collect();
        if chars.len() < kc.len() {
            continue;
        }
        for i in 0..=chars.len() - kc.len() {
            let matched = (0..kc.len()).all(|k| chars[i + k].eq_ignore_ascii_case(&kc[k]));
            if !matched {
                continue;
            }
            let before_ok = i == 0 || !is_ident_char(chars[i - 1]);
            let after_ok = i + kc.len() == chars.len() || !is_ident_char(chars[i + kc.len()]);
            if before_ok && after_ok {
                return Some(kw);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{classify_sql_error_class, decapsulate_copy_error};

    /// 防漂移（fail-loud，借鉴 deepseek-harness「提示词与工具注册表对齐」）：
    /// preamble 里反引号提及的 snake_case 标识符，要么是真实注册的工具名，
    /// 要么在 allowlist 里（非工具标识符：SQL 函数 / 参数名 / OKF 类别名 /
    /// 连接别名与示例视图名）。新增/改名工具、或在 preamble 里新写反引号
    /// 标识符导致两边都不在时，此测试失败——强制显式归类，防止提示词与
    /// 实际工具集漂移（历史教训：模型臆造不存在的工具名会被服务端整请求拒绝）。
    #[test]
    fn preamble_backtick_identifiers_are_real_tools() {
        use std::collections::HashSet;
        use rig_core::tool::Tool;

        // 与 runner.rs build_data_tools 一致的数据分析场景工具全集。
        let tools: HashSet<&str> = [
            crate::skill::builtin::GetCurrentTimeTool::NAME,
            crate::skill::search::SearchTool::NAME,
            super::execute_query::ExecuteQueryTool::NAME,
            super::list_tables::ListTablesTool::NAME,
            super::describe_table::DescribeTableTool::NAME,
            super::sample_data::SampleDataTool::NAME,
            super::render_chart::RenderChartTool::NAME,
            super::load_okf_knowledge::LoadOkfKnowledgeTool::NAME,
            super::write_okf_knowledge::WriteOkfKnowledgeTool::NAME,
            super::search_okf_knowledge::SearchOkfKnowledgeTool::NAME,
            super::create_view::CreateViewTool::NAME,
            super::drop_object::DropObjectTool::NAME,
            super::list_connections::ListConnectionsTool::NAME,
            super::list_remote_tables::ListRemoteTablesTool::NAME,
            super::register_table::RegisterTableTool::NAME,
            super::read_okf_metadata::ReadOkfMetadataTool::NAME,
            super::update_okf_metadata::UpdateOkfMetadataTool::NAME,
            super::list_okf_knowledge::ListOkfKnowledgeTool::NAME,
            super::delete_okf_knowledge::DeleteOkfKnowledgeTool::NAME,
            super::rename_okf_knowledge::RenameOkfKnowledgeTool::NAME,
        ]
        .into();

        let allowlist: HashSet<&str> = [
            "postgres_query",    // DuckDB 表函数，不是工具
            "filter",            // list_remote_tables 参数名
            "all",               // load_okf_knowledge heading 参数值
            "heading",           // load_okf_knowledge 参数名
            "concepts",          // OKF 类别名（其余同名条目同理）
            "tables",
            "views",
            "selections",
            "playbooks",
            "users",
            "db_",               // 连接别名前缀
            "demo",
            "db_demo",           // 示例连接别名
            "v_xxx",             // 示例视图短名
            "v_orders",
            "right_y_fields",    // render_chart 参数名
            "y_field_labels",
        ]
        .into();

        let strip_code_fences = |s: &str| -> String {
            let mut out = String::new();
            let mut rest = s;
            while let Some(start) = rest.find("```") {
                out.push_str(&rest[..start]);
                let after = &rest[start + 3..];
                match after.find("```") {
                    Some(end) => rest = &after[end + 3..],
                    None => rest = "",
                }
            }
            out.push_str(rest);
            out
        };

        let is_identifier = |t: &str| {
            let mut chars = t.chars();
            matches!(chars.next(), Some('a'..='z'))
                && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
        };

        for (name, preamble) in [
            ("PREAMBLE", crate::usage::PREAMBLE),
            ("DATA_ANALYSIS_PREAMBLE", crate::usage::DATA_ANALYSIS_PREAMBLE),
        ] {
            // split('`') 的奇数下标段即反引号包裹的 token。
            for (i, token) in strip_code_fences(preamble).split('`').enumerate() {
                if i % 2 == 0 {
                    continue;
                }
                let token = token.trim();
                if !is_identifier(token) {
                    continue;
                }
                assert!(
                    tools.contains(token) || allowlist.contains(token),
                    "{name} 提到标识符 `{token}`：既不是注册工具，也不在 allowlist。\
                     若是新工具请同步 runner 的工具构造与 NAME 常量；若是新的非工具\
                     标识符请加入 allowlist 并注明用途。"
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // COPY binary 错误剥壳（2026-08-27 复盘：6 次报错根因被内层 SQL 淹没）。
    // 用例取自当次会话的真实错误原文。
    // ------------------------------------------------------------------

    #[test]
    fn decapsulates_prepare_copy_relation_error() {
        let raw = r#"Invalid Error: Failed to prepare COPY "COPY (SELECT "msg_type", "cost_sum" FROM (SELECT msg_type, SUM(total_cost) AS cost_sum FROM "default"."dws_account_total_cost_tag" GROUP BY msg_type) AS __unnamed_subquery  LIMIT 50) TO STDOUT (FORMAT "binary");": ERROR:  relation "default.dws_account_total_cost_tag" does not exist"#;
        let out = decapsulate_copy_error(raw);
        assert!(out.contains("relation") && out.contains("does not exist"), "got: {out}");
        assert!(!out.contains("COPY (SELECT"), "内层 SQL 应被剥掉: {out}");
        assert_eq!(classify_sql_error_class(&out), "table_missing");
    }

    #[test]
    fn decapsulates_prepare_copy_permission_error() {
        let raw = r#"Invalid Error: Failed to prepare COPY "COPY (SELECT "cnt" FROM (SELECT COUNT(*) AS cnt FROM "default"."dim_hotel_camping_cost_item") AS __unnamed_subquery  LIMIT 50) TO STDOUT (FORMAT "binary");": ERROR:  check permission for foreign table scan failed: failed to check permission:MaxCompute"#;
        let out = decapsulate_copy_error(raw);
        assert!(out.contains("check permission"), "got: {out}");
        assert!(!out.contains("COPY (SELECT"));
        assert_eq!(classify_sql_error_class(&out), "permission");
    }

    #[test]
    fn decapsulates_fetch_header_cast_error() {
        let raw = r#"IO Error: Failed to fetch header for COPY: ERROR:  invalid input syntax for integer: "2000.0" CONTEXT:  [query_id:1002017781610729626]"#;
        let out = decapsulate_copy_error(raw);
        assert!(out.starts_with("invalid input syntax for integer"), "got: {out}");
        assert_eq!(classify_sql_error_class(&out), "cast_error");
    }

    #[test]
    fn plain_errors_pass_through_unchanged() {
        let raw = "Binder Error: Failed to find column: pay_time";
        assert_eq!(decapsulate_copy_error(raw), raw);
        // 普通错误恰好含 `";": ` 序列但无 COPY 前缀时不误剥
        let tricky = "some error \";\": looks like terminator";
        assert_eq!(decapsulate_copy_error(tricky), tricky);
    }
}
