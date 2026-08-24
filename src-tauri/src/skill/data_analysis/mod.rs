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

/// 归并 SQL 执行错误到 compact class，供 logs 表与 transcript meta 共用。
/// 值域固定，后续 agent 调优 / 历史排查可直接按此维度过滤。
pub(crate) fn classify_sql_error_class(err: &str) -> &'static str {
    let lower = err.to_lowercase();
    if lower.contains("permission denied") || lower.contains("access denied") {
        "permission"
    } else if lower.contains("timeout") || lower.contains("interrupt") || lower.contains("已自动中断") {
        "timeout"
    } else if (lower.contains("does not exist") || lower.contains("not found"))
        && lower.contains("column")
    {
        "column_missing"
    } else if lower.contains("does not exist") || lower.contains("not found") {
        "table_missing"
    } else if lower.contains("syntax") || lower.contains("parser") || lower.contains("binder") {
        "syntax"
    } else {
        "other"
    }
}
