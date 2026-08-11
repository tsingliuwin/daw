//! Skill 机制：标准化的可插拔能力包。
//!
//! 一个 Skill = 领域 preamble + 元数据（id/name）。该 skill 的 Tool 在 runner 的
//! `build_tools` 里用具体类型构造（rig 的 `Tool` trait 不是 dyn-compatible，
//! 无法用 `Box<dyn Tool>` 统一收集，因此工具注册用编译期代码，
//! preamble 拼装用运行时 SkillRegistry）。
//!
//! ## 基座 vs Skill
//!
//! - **基座内置**：agent runner、wire 协议、workspace/task 管理、LLM 配置。
//!   基座内置工具：get_current_time（时间解析）、确认/中止机制。
//! - **Skill**：领域特化能力，可插拔。每个 skill 自带领域 preamble，
//!   其工具在 runner 里按 skill id 条件构造。
//!
//! ## 扩展方式
//!
//! 新增一个 skill 的步骤：
//! 1. 实现 `Skill` trait（提供 id/name/preamble）
//! 2. 在 runner 的 `build_tools` 里加 `match skill_id` 分支，构造该 skill 的工具
//! 3. 在 `SkillRegistry::default()` 里 `register()` 该 skill
//! 未来可从 `~/.aioa/skills/` 动态扫描 preamble，工具构造用 trait 泛型或 WASM。

pub mod builtin;
pub mod context;
pub mod data_analysis;
pub mod registry;
pub mod search;

pub use context::SkillContext;
pub use registry::SkillRegistry;

/// 一个可插拔的能力包。
///
/// Skill 自带领域 preamble（注入 LLM 的领域指令），工具在 runner 里按
/// skill id 条件构造（rig Tool trait 非 dyn-compatible）。
pub trait Skill: Send + Sync {
    /// Skill 唯一标识（如 `"oa"`、`"finance"`）。
    fn id(&self) -> &str;
    /// Skill 显示名（如 `"OA 办公"`）。
    fn name(&self) -> &str;
    /// 领域 preamble（注入 LLM，拼接在通用基座 preamble 之后）。
    fn preamble(&self) -> &str;
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
