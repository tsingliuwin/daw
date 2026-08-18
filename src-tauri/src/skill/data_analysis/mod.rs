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
pub mod load_okf_block;
pub mod register_table;
pub mod rename_okf_knowledge;
pub mod render_chart;
pub mod read_okf_metadata;
pub mod sample_data;
pub mod search_okf_knowledge;
pub mod update_okf_metadata;
pub mod write_okf_block;
