//! DuckDB 引擎层——数据分析场景的查询执行 + 外部数据源 ATTACH。
//!
//! P1 只迁了查询执行（execute.rs）和外部库 ATTACH（attach.rs）。
//! DuckLake / 文件扫描 / 建表加工层（lakemind 的 lake.rs/register.rs/scan.rs）
//! 留待后续阶段（DDL 加工）再迁。

pub mod attach;
pub mod execute;
pub mod lake;

use crate::model::DataSourceConfig;

/// 软超时（秒）：超过后通过 InterruptHandle 中断查询。
pub const QUERY_TIMEOUT_SECS: u64 = 60;

/// 硬超时（秒）：超过后强制中断查询。工具层用 `tokio::time::timeout` 兜底。
pub const QUERY_HARD_TIMEOUT_SECS: u64 = 120;

/// 从 `~/.aioa/settings.json` 读 `dataSources` 数组。
/// 读取失败或未配置时返回空列表（不报错——数据分析工具调时会提示"未配置数据源"）。
pub fn load_data_sources() -> Vec<DataSourceConfig> {
    let mut path = match crate::db::get_aioa_dir() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    path.push("settings.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    settings
        .get("dataSources")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}
