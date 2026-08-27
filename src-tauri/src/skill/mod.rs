//! Skill 场景机制：按任务场景（Scenario）组织工具集与系统提示词。
//!
//! rig 的 `Tool` trait 不是 dyn-compatible，无法 `Box<dyn Tool>` 统一收集，
//! 因此 runner 按 `Scenario` 在编译期构造各场景的具体工具元组；系统提示词
//! 分两层组装（借鉴 deepseek-harness 的静态/动态分离）：静态 preamble 在
//! `usage.rs` 按场景二选一，每轮变化的事实（时间、OKF 大纲）由 runner 以
//! `<runtime_context>` 快照随用户消息下发。早期的运行时 SkillRegistry
//! preamble 拼装方案已移除——它从未接线，且与 runner 的实际组装并存易致漂移。
//!
//! ## 基座 vs 场景
//!
//! - **基座内置**：agent runner、wire 协议、workspace/task 管理、LLM 配置。
//!   基座内置工具：get_current_time（时间解析）、search（联网搜索）。
//! - **场景**：领域特化能力（当前为数据分析 data_analysis），自带领域
//!   preamble 与工具集，在 runner 里按 Scenario 条件构造。

pub mod builtin;
pub mod context;
pub mod data_analysis;
pub mod search;

/// Per-task workspace pointer injected into data-analysis tools.
///
/// Replaces the old shared `AppState.workspace_path`/`workspace_dir` fields,
/// which were clobbered whenever multiple workspaces ran concurrently. Each
/// tool reads its task's workspace from this owned clone, so concurrent
/// cross-workspace tasks never read each other's workspace.
#[derive(Clone)]
pub struct WorkspaceRef {
    /// Workspace key (`workspaces.path`), e.g. "DefaultProject".
    pub path: String,
    /// Absolute workspace directory `~/.daw/<path>`.
    pub dir: std::path::PathBuf,
}

/// 任务场景。在任务创建时绑定（存 task.kind），决定 preamble、工具集、交互模式。
/// runner 根据 scenario 编译期构造不同工具集（绕过 rig Tool 非 dyn-compatible 限制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// 通用对话（get_current_time + search）。
    General,
    /// 数据分析（get_current_time + execute_query / list_tables / describe_table / sample_data）。
    DataAnalysis,
}

impl Scenario {
    /// 从 task 的 kind 字段解析场景。
    pub fn from_kind(kind: &str) -> Self {
        if kind == "data_analysis" {
            Scenario::DataAnalysis
        } else {
            Scenario::General
        }
    }
}
