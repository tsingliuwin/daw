//! Agent tools — 已迁移到 skill 机制。
//!
//! 基座内置工具（get_current_time）在 `skill/builtin.rs`。
//! 领域 skill 的工具在各自 skill 模块里，由 runner 的 build_tools 条件注册。

// 此模块保留为占位——工具定义已迁移到 skill/builtin.rs。
// runner.rs 通过 crate::skill::builtin::GetCurrentTimeTool 引用基座工具。
