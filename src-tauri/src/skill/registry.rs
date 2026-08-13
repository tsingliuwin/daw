//! SkillRegistry — 收集所有已注册的 Skill，拼装领域 preamble。
//!
//! runner.rs 从 registry 拿 `combined_preamble()`（通用基座 + 各 skill 领域）。
//! 工具注册在 runner 的 build_tools 里用具体类型构造（rig Tool 非 dyn-compatible）。

use super::Skill;
use crate::usage;

/// Skill 注册表。启动时初始化，注册基座 + 各 skill。
#[allow(dead_code)]
pub struct SkillRegistry {
    skills: Vec<Box<dyn Skill>>,
}

#[allow(dead_code)]
impl SkillRegistry {
    pub fn new() -> Self {
        Self { skills: Vec::new() }
    }

    /// 注册一个 skill。
    pub fn register(&mut self, skill: Box<dyn Skill>) {
        tracing::info!(category = "system", "skill registered: {} ({})", skill.name(), skill.id());
        self.skills.push(skill);
    }

    /// 通用基座 preamble + 所有已注册 skill 的领域 preamble 拼接。
    pub fn combined_preamble(&self) -> String {
        let mut parts = vec![usage::PREAMBLE.to_string()];
        for skill in &self.skills {
            let p = skill.preamble();
            if !p.is_empty() {
                parts.push(format!("# 领域指令：{}\n{}", skill.name(), p));
            }
        }
        parts.join("\n\n---\n\n")
    }

    /// 已注册 skill 的 id 列表（runner 据此条件构造工具）。
    pub fn enabled_skill_ids(&self) -> Vec<&str> {
        self.skills.iter().map(|s| s.id()).collect()
    }

    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}
