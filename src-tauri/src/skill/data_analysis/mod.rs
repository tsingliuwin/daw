//! 数据分析场景工具——从 lakemind 迁移的查询闭环。
//!
//! P1 含 4 个只读查询工具（execute_query / list_tables / describe_table /
//! sample_data）。DDL 加工工具（create_table / create_view / drop_object）
//! 和 OKF 知识库工具留待后续阶段。

pub mod create_view;
pub mod describe_table;
pub mod drop_object;
pub mod execute_query;
pub mod list_connections;
pub mod list_remote_tables;
pub mod list_tables;
pub mod load_okf_block;
pub mod register_table;
pub mod render_chart;
pub mod sample_data;
pub mod search_okf_recipes;
pub mod write_okf_block;
