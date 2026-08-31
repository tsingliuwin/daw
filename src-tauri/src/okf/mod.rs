//! OKF（Open Knowledge Format）— 独立知识管理模块。
//!
//! 给 agent 用的知识库：跨会话继承业务背景、表字段释义、关联、排障经验。
//!
//! 设计要点：
//! - **统一 API 契约**（[`Okf`] facade）：调用方只依赖 facade 方法，不伸手进文件
//!   内部；存储格式/解析/缓存可在模块内自由演进。
//! - **大纲 = 自动扫描唯一源**（[`catalog::summary`]），无 index.md 维护负担。
//! - **状态归 table_registry**（结构化），OKF 文件只存业务知识——不双写。
//! - **可测根基**：`OkfPaths`/`Versioner`/`Clock` 可注入（tempdir + 固定时钟 + noop 版本）。
//!
//! 子模块：[`model`]（类型）、[`paths`]（路径）、[`frontmatter`]（YAML）、
//! [`markdown`]（正文）、[`store`]（文件 I/O）、[`catalog`]（大纲/搜索）。
//! 文件格式：YAML frontmatter + Markdown body。

pub mod catalog;
pub mod frontmatter;
pub mod markdown;
pub mod model;
pub mod paths;
pub mod similarity;
pub mod store;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use model::{Category, ColumnSemantics, SearchHit};
use paths::OkfPaths;

// ===========================================================================
// 公共 facade + 可注入的 Versioner / Clock（DI，测试根基）
// ===========================================================================

/// 文件版本控制（git 提交）。可注入 noop 用于测试。
pub trait Versioner: Send + Sync {
    /// 在 `repo_root`（okf 根目录）仓库内提交 `file_path`，首次自动 git init。
    fn commit(&self, repo_root: &Path, file_path: &Path, commit_msg: &str);
}

/// 真实 git 版本控制（复用 [`run_git_commit`]）。
pub struct GitVersioner;
impl Versioner for GitVersioner {
    fn commit(&self, repo_root: &Path, file_path: &Path, commit_msg: &str) {
        run_git_commit(repo_root, file_path, commit_msg);
    }
}

/// 测试用：什么都不做。
#[allow(dead_code)]
pub struct NoopVersioner;
impl Versioner for NoopVersioner {
    fn commit(&self, _repo_root: &Path, _file_path: &Path, _commit_msg: &str) {}
}

/// 时钟。可注入固定值用于测试。
pub trait Clock: Send + Sync {
    fn now_ts(&self) -> String;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ts(&self) -> String {
        current_timestamp()
    }
}

/// 固定时钟（测试用）。
#[allow(dead_code)]
pub struct FixedClock(pub String);
impl Clock for FixedClock {
    fn now_ts(&self) -> String {
        self.0.clone()
    }
}

/// OKF 模块 facade。持有路径 + 版本器 + 时钟，对外提供统一 API。
///
/// 调用方只依赖此结构的方法，不再伸手进文件内部——内部实现可自由演进。
/// 生产用 [`Okf::production`]，测试用 [`Okf::new`] 注入 tempdir + NoopVersioner + FixedClock。
pub struct Okf {
    pub paths: OkfPaths,
    pub versioner: Arc<dyn Versioner>,
    pub clock: Arc<dyn Clock>,
}

impl Okf {
    /// 生产构造：真 git + 系统时钟，根 = `~/.daw`。
    pub fn production() -> Self {
        Self::with_paths(OkfPaths::production())
    }

    /// 用指定 paths + 真 git + 系统时钟。
    pub fn with_paths(paths: OkfPaths) -> Self {
        Self {
            paths,
            versioner: Arc::new(GitVersioner),
            clock: Arc::new(SystemClock),
        }
    }

    /// 测试构造：完全注入。
    #[allow(dead_code)]
    pub fn new(paths: OkfPaths, versioner: Arc<dyn Versioner>, clock: Arc<dyn Clock>) -> Self {
        Self { paths, versioner, clock }
    }

    // ---- 生命周期 ----

    /// 初始化全局 OKF（建 concepts/users 目录）。
    pub fn init_global(&self) -> Result<(), String> {
        store::init_global(&self.paths)
    }

    /// 初始化单个工作区 OKF（建标准子目录）。
    pub fn init_workspace(&self, ws: &str) -> Result<(), String> {
        store::init_workspace(&self.paths, ws)
    }

    /// 启动总初始化：全局 + 所有已注册工作区（幂等）。
    pub fn init_all(&self) -> Result<(), String> {
        self.init_global()?;
        for ws in crate::db::list_workspace_paths().unwrap_or_default() {
            if let Err(e) = self.init_workspace(&ws) {
                tracing::warn!(category = "okf", "工作区 {ws} OKF 初始化失败: {e}");
            }
        }
        Ok(())
    }

    // ---- 读写 ----

    pub fn read(
        &self,
        ws: &str,
        category: Category,
        name: &str,
        heading: &str,
    ) -> Result<model::OkfReadOutcome, String> {
        store::read(&self.paths, ws, category, name, heading)
    }

    pub fn write(
        &self,
        ws: &str,
        category: Category,
        name: &str,
        heading: &str,
        content: &str,
        description: Option<&str>,
    ) -> Result<model::OkfWriteOutcome, String> {
        store::write(
            &self.paths,
            self.versioner.as_ref(),
            self.clock.as_ref(),
            ws,
            category,
            name,
            heading,
            content,
            description,
        )
    }

    pub fn delete(&self, ws: &str, name: &str) -> Result<bool, String> {
        store::delete(&self.paths, self.versioner.as_ref(), ws, name)
    }

    // ---- 知识整理（防重 / 合并 / 重命名） ----

    /// 指定知识文件是否已存在（写入防重守卫的第一道闸）。
    pub fn knowledge_exists(&self, ws: &str, category: Category, name: &str) -> bool {
        self.paths
            .category_dir(category.scope(), ws, category)
            .join(format!("{name}.md"))
            .exists()
    }

    /// 新建防重：同类目下与 (name, description) 疑似相似的既有条目，按分数降序。
    pub fn find_similar(
        &self,
        ws: &str,
        category: Category,
        name: &str,
        description: Option<&str>,
    ) -> Vec<similarity::SimilarCandidate> {
        similarity::find_similar_in_dir(
            &self.paths.category_dir(category.scope(), ws, category),
            name,
            description,
        )
    }

    /// 删除一条知识（任意类别，含全局 concepts/users）。
    /// `merge_into=Some(保留文件)` 时全库 `[[被删名]]` 内链改写指向保留文件。
    pub fn delete_knowledge(
        &self,
        ws: &str,
        category: Category,
        name: &str,
        merge_into: Option<&str>,
    ) -> Result<bool, String> {
        store::delete_doc(&self.paths, self.versioner.as_ref(), ws, category, name, merge_into)
    }

    /// 重命名知识文件 + 改 frontmatter title + 全库内链同步改写。返回新路径。
    pub fn rename_knowledge(
        &self,
        ws: &str,
        category: Category,
        old: &str,
        new: &str,
    ) -> Result<std::path::PathBuf, String> {
        store::rename_doc(&self.paths, self.versioner.as_ref(), self.clock.as_ref(), ws, category, old, new)
    }

    // ---- 骨架 ----

    pub fn ensure_table_skeleton(
        &self,
        ws: &str,
        table: &str,
        columns: &[model::ColumnInfo],
        row_count: Option<i64>,
    ) -> Result<bool, String> {
        store::ensure_table_skeleton(
            &self.paths,
            self.versioner.as_ref(),
            self.clock.as_ref(),
            ws,
            table,
            columns,
            row_count,
        )
    }

    pub fn ensure_view_skeleton(&self, ws: &str, view: &str, sql: &str) -> Result<bool, String> {
        store::ensure_view_skeleton(
            &self.paths,
            self.versioner.as_ref(),
            self.clock.as_ref(),
            ws,
            view,
            sql,
        )
    }

    // ---- 语义 / 大纲 / 搜索 ----

    pub fn column_semantics(&self, ws: &str, name: &str) -> ColumnSemantics {
        store::column_semantics(&self.paths, ws, name)
    }

    /// 结构化语义解析（含关联内链，step6 catalog 用）。
    #[allow(dead_code)]
    pub fn table_semantics(&self, ws: &str, name: &str) -> model::TableSemantics {
        store::table_semantics(&self.paths, ws, name)
    }

    /// 读取一个 OKF 文件的结构化元数据（frontmatter 字段）。
    pub fn read_metadata(
        &self,
        ws: &str,
        category: Category,
        name: &str,
    ) -> Result<frontmatter::Frontmatter, String> {
        store::read_metadata(&self.paths, ws, category, name)
    }

    /// 只改 frontmatter 指定字段（不动正文，自动刷 updated_at）。
    pub fn update_metadata(
        &self,
        ws: &str,
        category: Category,
        name: &str,
        fields: &[(String, String)],
    ) -> Result<(), String> {
        store::update_metadata(
            &self.paths,
            self.versioner.as_ref(),
            self.clock.as_ref(),
            ws,
            category,
            name,
            fields,
        )
    }

    /// 生成注入 preamble 的大纲（表清单由调用方从 table_registry 传入）。
    pub fn catalog_summary(&self, ws: &str, table_entries: &[crate::model::TableRegistryEntry]) -> String {
        catalog::summary(&self.paths, ws, table_entries)
    }

    /// 生成带完整元数据的大纲（list_okf_knowledge 工具用，比 summary 丰富）。
    pub fn catalog_outline(&self, ws: &str, table_entries: &[crate::model::TableRegistryEntry]) -> String {
        catalog::outline(&self.paths, ws, table_entries)
    }

    pub fn search(&self, ws: &str, query: &str) -> Vec<SearchHit> {
        catalog::search(&self.paths, ws, query)
    }
}

// ===========================================================================
// 生产实现依赖：git 版本 + UTC 时间戳（无 chrono）
// ===========================================================================

/// 在 `okf_dir` 仓库内提交 `file_path`（首次自动 git init + 换行配置）。
///
/// 失败不再静默：git 提交是知识库变更追溯的根基（复盘实锤，静默失败曾让
/// 80+ 表骨架从未入库，历史断档无人知晓），失败必须进日志。git 的
/// `add`/`commit` 撞 `index.lock` 会整步失败——骨架生成常连发几十次提交，
/// 进程内用互斥锁串行化（复盘脚本对仓库只读，跨进程冲突可忽略）。
fn run_git_commit(okf_dir: &Path, file_path: &Path, commit_msg: &str) {
    use std::process::Command;
    use std::sync::Mutex;
    #[cfg(target_os = "windows")]
    use std::os::windows::process::CommandExt;

    static GIT_LOCK: Mutex<()> = Mutex::new(());
    let _guard = GIT_LOCK.lock();

    let git_dir = okf_dir.join(".git");
    if !git_dir.exists() {
        let mut cmd = Command::new("git");
        cmd.arg("init").current_dir(okf_dir);
        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(0x08000000);
        }
        let _ = cmd.status();

        let _ = fs::write(okf_dir.join(".gitignore"), ".DS_Store\n");
        let _ = fs::write(
            okf_dir.join(".gitattributes"),
            "* text=auto eol=lf\n*.md text eol=lf\n",
        );

        let mut cmd = Command::new("git");
        cmd.args(["config", "core.autocrlf", "false"]).current_dir(okf_dir);
        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(0x08000000);
        }
        let _ = cmd.status();
    }
    if let Ok(rel_path) = file_path.strip_prefix(okf_dir) {
        let mut cmd = Command::new("git");
        cmd.arg("add").arg(rel_path).current_dir(okf_dir);
        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(0x08000000);
        }
        let add_ok = cmd.status().map(|s| s.success()).unwrap_or(false);

        let mut cmd = Command::new("git");
        cmd.arg("commit").arg("-m").arg(commit_msg).current_dir(okf_dir);
        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(0x08000000);
        }
        let commit_ok = cmd.status().map(|s| s.success()).unwrap_or(false);
        // 「nothing to commit」（内容与上次完全一致的幂等写）不算失败：提交
        // 失败且工作区已干净 = 本来就没有需要落库的差异。
        if !add_ok || (!commit_ok && !git_worktree_clean(okf_dir)) {
            tracing::warn!(
                category = "system",
                "OKF git 提交未生效（add={add_ok} commit={commit_ok}），知识变更历史断档风险：{commit_msg}"
            );
        }
    }
}

/// 工作区是否干净（`git status --porcelain` 无输出）。用于把幂等写的
/// 「nothing to commit」从失败里区分出来。
fn git_worktree_clean(okf_dir: &Path) -> bool {
    use std::process::Command;
    Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(okf_dir)
        .output()
        .map(|o| o.stdout.is_empty())
        .unwrap_or(false)
}

/// 当前 UTC 时间戳 `YYYY-MM-DDTHH:MM:SSZ`（手写日期换算，不依赖 chrono）。
fn current_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let mut year = 1970;
    let mut days_rem = days;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let size = if leap { 366 } else { 365 };
        if days_rem >= size {
            days_rem -= size;
            year += 1;
        } else {
            break;
        }
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1;
    let mut day = days_rem as u32 + 1;
    for &md in &month_days {
        if day > md {
            day -= md;
            month += 1;
        } else {
            break;
        }
    }
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}
