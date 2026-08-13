//! OKF（Open Knowledge Format）— 三层知识库。
//!
//! 全局（~/.aioa/okf/）：跨工作区共享的通用业务概念 + 用户背景
//! 工作区（~/.aioa/<workspace>/okf/）：表探索状态 + 字段释义 + 关联关系
//!
//! 全局走 index.md 渐进式披露（agent 按需 load_okf_block）。
//! 工作区走 memory summary 每轮注入 preamble（agent 自动继承）。
//!
//! 文件格式：YAML frontmatter + Markdown body。
//! 表探索状态记录在 frontmatter 的 status 字段：
//!   available / unavailable_permanent / unavailable_temporary

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Seed content for the global OKF `index.md` — the progressive-disclosure
/// outline the agent reads via `load_okf_block(category="global", name="index")`.
const GLOBAL_INDEX_SEED: &str = r#"---
type: Global Knowledge Index
description: 全局知识库目录大纲（渐进式披露入口，agent 按需精读）
---

# 全局知识库

跨工作区共享的知识。agent 先读本大纲，再按名精读具体文件，无需全部加载。

## 通用业务概念（concepts/）
用 `load_okf_block(category="concepts", name="<概念名>", heading="<标题>")` 精读。
- （暂无，使用中由 agent 或用户沉淀）

## 用户背景（users/）
用 `load_okf_block(category="users/default", name="<名称>", heading="<标题>")` 精读。
- （暂无，使用中由 agent 或用户沉淀）
"#;

/// Seed content for each workspace OKF `index.md`. Complements the
/// auto-injected memory summary with a manually-refinable framework that
/// documents every standard subdir's purpose.
const WORKSPACE_INDEX_SEED: &str = r#"---
type: Workspace Knowledge Index
description: 工作区知识库索引（表探索状态自动注入 preamble，本文件供手动沉淀补充）
---

# 工作区知识库

本工作区的数据探索知识。表/视图的探索状态由系统自动跟踪并每轮注入 memory summary，
此处记录补充的业务上下文、字段释义、关联关系与排障配方。

## 表（tables/）
每张已探索的表自动生成 `tables/<表名>.md` 骨架（字段 Schema + 关联关系），agent 后续补充业务释义。
- （暂无已探索的表）

## 视图（views/）
注册的本地视图 `v_xxx` 定义存于 `views/<视图名>.md`。
- （暂无）

## 数据源（sources/）
- （暂无）

## 工作区概念（concepts/）
工作区级业务概念（跨工作区共享的概念在全局 okf/concepts/）。
- （暂无）

## 排障配方（pipelines/specific/）
数据清洗、加载等排障记录 `pipelines/specific/<名称>.md`，可用 `search_okf_recipes` 检索。
- （暂无）
"#;

/// Get the root OKF directory for a workspace: `<ws_path>/okf`
pub fn get_okf_dir(ws_path: &str) -> PathBuf {
    Path::new(ws_path).join("okf")
}

/// Get the global OKF directory: `~/.aioa/okf/`
pub fn get_global_okf_dir() -> Result<PathBuf, String> {
    let mut path = crate::db::get_aioa_dir()?;
    path.push("okf");
    Ok(path)
}

/// Ensure global OKF dirs exist (concepts/ + users/<user_id>/).
pub fn ensure_global_okf_dirs(user_id: &str) -> Result<PathBuf, String> {
    let okf_dir = get_global_okf_dir()?;
    for sub in &["concepts", &format!("users/{user_id}")] {
        let path = okf_dir.join(sub);
        if !path.exists() {
            fs::create_dir_all(&path).map_err(|e| format!("无法创建目录 {:?}: {e}", path))?;
        }
    }
    Ok(okf_dir)
}

/// Ensure that all standard OKF subdirectories exist.
pub fn ensure_okf_dirs(ws_path: &str) -> Result<PathBuf, String> {
    let okf_dir = get_okf_dir(ws_path);
    let subdirs = ["tables", "views", "sources", "concepts", "pipelines/specific"];
    for sub in &subdirs {
        let path = okf_dir.join(sub);
        if !path.exists() {
            fs::create_dir_all(&path).map_err(|e| format!("无法创建目录 {:?}: {}", path, e))?;
        }
    }
    Ok(okf_dir)
}

/// Resolve a `workspaces.path` value to its actual filesystem directory.
///
/// Custom workspaces (registered via `add_workspace`) store an absolute path
/// picked by the directory dialog, returned as-is. The default workspace
/// stores the relative key `"DefaultProject"` (seeded by `init_global_db`),
/// resolved to `~/.aioa/DefaultProject`.
pub fn resolve_workspace_dir(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        Ok(p)
    } else {
        Ok(crate::db::get_aioa_dir()?.join(path))
    }
}

/// 若 `<okf_dir>/index.md` 不存在，写入种子内容并 git 提交（幂等）。
/// 已存在不覆盖，保留后续沉淀的内容。run_git_commit 只 add 传入的具体文件，
/// 故种子必须在写入时一并提交，否则后续 write_okf_block 触发 git init 时
/// 也不会把 index.md 纳入版本控制。
fn seed_index_md(okf_dir: &Path, seed: &str, commit_msg: &str) -> Result<(), String> {
    let index_path = okf_dir.join("index.md");
    if !index_path.exists() {
        fs::write(&index_path, seed).map_err(|e| format!("写入 index.md 失败: {e}"))?;
        run_git_commit(okf_dir, &index_path, commit_msg);
    }
    Ok(())
}

/// 为单个工作区初始化 OKF：标准目录 + 种子 index.md + git 版本化（幂等）。
/// 供 `init_okf`（启动遍历所有工作区）与 `add_workspace`（新建工作区）共用。
pub fn init_workspace_okf(ws_path: &str) -> Result<PathBuf, String> {
    let okf_dir = ensure_okf_dirs(ws_path)?;
    seed_index_md(&okf_dir, WORKSPACE_INDEX_SEED, "Bootstrap workspace OKF index")?;
    Ok(okf_dir)
}

/// 首次启动时确保 OKF 全局 + 所有已注册工作区的目录结构完整（幂等）。
///
/// 独立于 DuckDB：即使 DuckLake 扩展未就绪，OKF 仍可读写。由 `lib::run()`
/// 在 `init_global_db` 之后调用，保证 agent 首次读取 `load_okf_block("",
/// "index")` 时不报"全局知识库目录尚不存在"，且每个 OKF（全局 + 各工作区）
/// 都具备标准目录、种子 index.md 与初始内容，后续再按知识沉淀补充修改。
pub fn init_okf() -> Result<(), String> {
    // 1) 全局 OKF：concepts/ + users/default/ + 种子 index.md（渐进式披露入口）。
    let global_dir = ensure_global_okf_dirs("default")?;
    seed_index_md(&global_dir, GLOBAL_INDEX_SEED, "Bootstrap global OKF index")?;

    // 2) 所有已注册工作区的 OKF（DefaultProject + 自定义工作区）。
    //    workspaces.path：自定义工作区是绝对路径（目录选择器），DefaultProject
    //    是相对键（init_global_db 种子），由 resolve_workspace_dir 统一解析。
    //    逐工作区幂等初始化（目录 + 种子 index.md + git），单个失败不阻断其余
    //    （如某工作区目录在不可访问的移动盘）。此处 tracing 尚未安装，用 eprintln。
    match crate::db::list_workspace_paths() {
        Ok(paths) => {
            for p in paths {
                let ws_str = match resolve_workspace_dir(&p) {
                    Ok(d) => d.to_string_lossy().to_string(),
                    Err(e) => {
                        eprintln!("Failed to resolve workspace dir for OKF init '{p}': {e}");
                        continue;
                    }
                };
                if let Err(e) = init_workspace_okf(&ws_str) {
                    eprintln!("Failed to init OKF for workspace '{p}': {e}");
                }
            }
        }
        Err(e) => eprintln!("Failed to list workspaces for OKF init: {e}"),
    }

    Ok(())
}

/// 列信息三元组：(字段名, 物理类型, 是否允许空)。
pub type ColumnInfo = (String, String, bool);

/// Write/update physical table metadata under `tables/<table_name>.md`。
/// 生成空业务释义的骨架，agent 后续通过 write_okf_block 补充。
pub fn write_table_okf(
    ws_path: &str,
    table_name: &str,
    columns: &[ColumnInfo],
    row_count: Option<i64>,
) -> Result<(), String> {
    let okf_dir = ensure_okf_dirs(ws_path)?;
    let file_path = okf_dir.join("tables").join(format!("{table_name}.md"));
    if file_path.exists() {
        return Ok(()); // 已存在不覆盖（保留 agent 写入的业务释义）
    }

    let mut schema_table = String::new();
    schema_table.push_str("| 字段名 | 物理类型 | 业务释义 | 数据约束 |\n");
    schema_table.push_str("|---|---|---|---|\n");
    for (name, ty, nullable) in columns {
        let constraint = if *nullable { "" } else { "NOT NULL" };
        schema_table.push_str(&format!("| `{name}` | {ty} |  | {constraint} |\n"));
    }

    let row_count_str = row_count.map(|c| c.to_string()).unwrap_or_else(|| "未知".to_string());
    let body = format!(
        "---\n\
        type: DuckDB Table\n\
        title: {table_name} 物理数据表\n\
        timestamp: {ts}\n\
        ---\n\n\
        # 物理画像\n\
        - 行数估算: {row_count_str}\n\n\
        # 字段 Schema\n\
        {schema_table}\n\
        # 关联关系\n\
        - 暂无关联表（请手动编辑，例如 `- customer_id 关联 customers 表的 customer_id`）。\n",
        ts = current_timestamp(),
        row_count_str = row_count_str,
        schema_table = schema_table,
    );

    fs::write(&file_path, body).map_err(|e| format!("写入 table OKF 失败: {e}"))?;
    Ok(())
}

/// Extract the text block under a heading from markdown content.
pub fn parse_okf_block_from_content(content: &str, heading: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut block_content = Vec::new();
    let mut recording = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading_text = trimmed.trim_start_matches('#').trim();
            if recording {
                break;
            }
            if heading_text.eq_ignore_ascii_case(heading) {
                recording = true;
                continue;
            }
        }
        if recording {
            block_content.push(line);
        }
    }

    if recording {
        Some(block_content.join("\n").trim().to_string())
    } else {
        None
    }
}

/// Read a specific heading block from an OKF file.
/// concepts/users 类别：先查工作区，找不到再查全局。
/// tables/views/sources/pipelines 类别：只查工作区。
/// 特殊（索引读取，返回整个 index.md 全文，忽略 heading）：
///   category="workspace" + name="index" → 当前工作区 index.md（<ws>/okf/index.md）
///   category="global"    + name="index" → 全局 index.md（~/.aioa/okf/index.md）
/// category 必须显式为 workspace 或 global；空字符串已废弃（不再是全局别名）。
pub fn read_okf_block(
    ws_path: &str,
    category: &str,
    name: &str,
    heading: &str,
) -> Result<String, String> {
    // 索引读取：返回整个 index.md 全文（heading 被忽略）。仅 workspace/global 有效。
    if name == "index" && (category == "workspace" || category == "global") {
        let (index_path, scope) = if category == "workspace" {
            (get_okf_dir(ws_path).join("index.md"), "工作区")
        } else {
            let global_dir = get_global_okf_dir().map_err(|e| format!("全局 OKF 目录不可用: {e}"))?;
            (global_dir.join("index.md"), "全局")
        };
        if !index_path.exists() {
            return Err(format!("{scope}知识库 index.md 尚不存在。"));
        }
        let content = fs::read_to_string(&index_path).map_err(|e| format!("读取 index.md 失败: {e}"))?;
        return Ok(content);
    }
    // category="" + name=index 已废弃：明确报错，避免静默回退到正常路径
    // 把工作区 index.md 当 heading 块读（隐蔽的魔法行为）。
    if name == "index" && category.is_empty() {
        return Err("读取索引需显式指定 category=\"workspace\" 或 \"global\"（空字符串已废弃）。".to_string());
    }

    let okf_dir = get_okf_dir(ws_path);
    let requested_path = okf_dir.join(category).join(format!("{name}.md"));
    let mut file_path = requested_path.clone();
    if !file_path.exists() {
        // concepts/users 类别：fallback 到全局。
        if category == "concepts" || category.starts_with("users") {
            if let Ok(global_dir) = get_global_okf_dir() {
                let global_path = global_dir.join(category).join(format!("{name}.md"));
                if global_path.exists() {
                    file_path = global_path;
                }
            }
        }
        // tables/views/sources 类别：工作区内 fallback。
        if !file_path.exists() && (category == "tables" || category == "views" || category == "sources") {
            let candidates = [
                okf_dir.join("tables").join(format!("{name}.md")),
                okf_dir.join("views").join(format!("{name}.md")),
                okf_dir.join("sources").join(format!("{name}.md")),
            ];
            for c in &candidates {
                if c.exists() {
                    file_path = c.clone();
                    break;
                }
            }
        }
        if !file_path.exists() {
            return Err(format!("文件不存在: {:?}", requested_path));
        }
    }

    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("读取文件失败: {e}"))?;

    parse_okf_block_from_content(&content, heading)
        .ok_or_else(|| format!("未找到标题为 '{heading}' 的板块"))
}

/// Write/update a heading block in an OKF file.
/// If the file doesn't exist, creates it with a minimal frontmatter stub.
/// After writing, runs `git commit` for version history.
pub fn write_okf_block(
    ws_path: &str,
    category: &str,
    name: &str,
    heading: &str,
    new_content: &str,
) -> Result<(), String> {
    // concepts/users 写全局，其他写工作区。
    let (okf_dir, doc_type) = if category == "concepts" || category.starts_with("users") {
        let global_dir = ensure_global_okf_dirs("default")?;
        let dt = if category.starts_with("users") { "User Profile" } else { "Business Concept" };
        (global_dir, dt)
    } else {
        let ws_dir = ensure_okf_dirs(ws_path)?;
        let dt = match category {
            "tables" => "DuckDB Table",
            "views" => "DuckDB View",
            "sources" => "Data Source",
            _ => "Concept",
        };
        (ws_dir, dt)
    };
    let file_path = okf_dir.join(category).join(format!("{name}.md"));

    let content = if file_path.exists() {
        fs::read_to_string(&file_path).map_err(|e| format!("读取文件失败: {e}"))?
    } else {
        format!(
            "---\ntype: {doc_type}\ntitle: {name}\ndescription: 自动初始化的 OKF 文档\n---\n"
        )
    };

    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines: Vec<String> = Vec::new();
    let mut skipped = false;
    let mut written = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading_text = trimmed.trim_start_matches('#').trim();
            if skipped {
                skipped = false;
            }
            if heading_text.eq_ignore_ascii_case(heading) {
                new_lines.push(line.to_string());
                new_lines.push(new_content.to_string());
                skipped = true;
                written = true;
                continue;
            }
        }
        if !skipped {
            new_lines.push(line.to_string());
        }
    }

    if !written {
        let level = if category == "concepts" { 2 } else { 1 };
        let prefix = "#".repeat(level);
        new_lines.push(String::new());
        new_lines.push(format!("{prefix} {heading}"));
        new_lines.push(new_content.to_string());
    }

    let clean_content = deduplicate_markdown(&new_lines.join("\n"));
    fs::write(&file_path, clean_content).map_err(|e| format!("写入文件失败: {e}"))?;

    run_git_commit(&okf_dir, &file_path, &format!("Update OKF: {category}/{name}"));
    Ok(())
}

/// git commit OKF 文件变更（首次写入时自动 git init）。
fn run_git_commit(okf_dir: &Path, file_path: &Path, commit_msg: &str) {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    use std::os::windows::process::CommandExt;

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
        let _ = cmd.status();

        let mut cmd = Command::new("git");
        cmd.arg("commit").arg("-m").arg(commit_msg).current_dir(okf_dir);
        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(0x08000000);
        }
        let _ = cmd.status();
    }
}

/// Remove duplicate heading blocks from markdown (outside YAML frontmatter).
pub fn deduplicate_markdown(content: &str) -> String {
    let mut clean_lines = Vec::new();
    let mut seen_headings = std::collections::HashSet::new();
    let mut skipping = false;
    let mut in_yaml = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            in_yaml = !in_yaml;
            clean_lines.push(line.to_string());
            continue;
        }
        if in_yaml {
            clean_lines.push(line.to_string());
            continue;
        }

        if trimmed.starts_with('#') {
            let heading_text = trimmed.trim_start_matches('#').trim().to_lowercase();
            if seen_headings.contains(&heading_text) {
                skipping = true;
            } else {
                seen_headings.insert(heading_text);
                skipping = false;
                clean_lines.push(line.to_string());
            }
        } else if !skipping {
            clean_lines.push(line.to_string());
        }
    }

    clean_lines.join("\n")
}

/// Parse table/view OKF to extract business title, column comments, and relations.
/// Returns (业务标题, {列名→释义}, 关联关系列表).
/// File lookup order: tables/ → views/ → sources/.
pub fn parse_column_semantics(
    ws_path: &str,
    table_name: &str,
) -> (Option<String>, HashMap<String, String>, Vec<String>) {
    let mut desc = None;
    let mut col_comments: HashMap<String, String> = HashMap::new();
    let mut relations = Vec::new();

    let okf_dir = get_okf_dir(ws_path);
    let mut file_path = okf_dir.join("tables").join(format!("{table_name}.md"));
    if !file_path.exists() {
        file_path = okf_dir.join("views").join(format!("{table_name}.md"));
    }
    if !file_path.exists() {
        file_path = okf_dir.join("sources").join(format!("{table_name}.md"));
    }
    if !file_path.exists() {
        return (desc, col_comments, relations);
    }

    let Ok(content) = fs::read_to_string(&file_path) else {
        return (desc, col_comments, relations);
    };

    desc = parse_yaml_field(&content, "title");

    let lines: Vec<&str> = content.lines().collect();
    let mut current_heading = "";
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            current_heading = trimmed.trim_start_matches('#').trim();
            continue;
        }

        if current_heading == "字段 Schema" || current_heading == "Column Schema" {
            if trimmed.starts_with('|')
                && !trimmed.contains("---|---")
                && !trimmed.contains("字段名")
                && !trimmed.contains("Column")
            {
                let parts: Vec<&str> = trimmed.split('|').map(|p| p.trim()).collect();
                if parts.len() >= 4 {
                    let col_name = parts[1].trim_matches('`').trim().to_string();
                    let meaning = parts[3].to_string();
                    if !col_name.is_empty() && !meaning.is_empty() {
                        col_comments.insert(col_name, meaning);
                    }
                }
            }
        } else if current_heading == "关联关系" || current_heading == "Relationships" {
            if trimmed.starts_with('-') || trimmed.starts_with('*') {
                let rel = trimmed
                    .trim_start_matches(|c| c == '-' || c == '*' || c == ' ')
                    .to_string();
                if !rel.is_empty() {
                    relations.push(rel);
                }
            }
        }
    }

    (desc, col_comments, relations)
}

/// Helper to parse a single YAML frontmatter field value.
pub fn parse_yaml_field(content: &str, field: &str) -> Option<String> {
    let mut in_yaml = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            if in_yaml {
                break;
            } else {
                in_yaml = true;
                continue;
            }
        }
        if in_yaml {
            let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
            if parts.len() == 2 && parts[0].trim().eq_ignore_ascii_case(field) {
                return Some(parts[1].trim().to_string());
            }
        }
    }
    None
}

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
    // days_rem 是当年第几天（0-based），换算成月日。
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

/// Delete an OKF file for a given object name (checks tables/views/sources).
pub fn delete_okf_file(ws_path: &str, name: &str) {
    let okf_dir = get_okf_dir(ws_path);
    for dir in &["tables", "views", "sources"] {
        let path = okf_dir.join(dir).join(format!("{name}.md"));
        if path.exists() {
            let _ = fs::remove_file(&path);
            run_git_commit(&okf_dir, &path, &format!("Delete OKF: {dir}/{name}"));
        }
    }
}

/// Write a view definition OKF skeleton (SQL + dependency hint).
pub fn write_view_okf(ws_path: &str, view_name: &str, select_sql: &str) -> Result<(), String> {
    let okf_dir = ensure_okf_dirs(ws_path)?;
    let file_path = okf_dir.join("views").join(format!("{view_name}.md"));
    if file_path.exists() {
        return Ok(()); // 已存在不覆盖
    }
    let body = format!(
        "---\n\
        type: DuckDB View\n\
        title: {view_name} 逻辑视图\n\
        timestamp: {ts}\n\
        ---\n\n\
        # 视图 SQL 定义\n\
        ```sql\n\
        {select_sql}\n\
        ```\n",
        ts = current_timestamp(),
        select_sql = select_sql,
    );
    fs::write(&file_path, body).map_err(|e| format!("写入 view OKF 失败: {e}"))?;
    Ok(())
}

/// 生成工作区 OKF 的 memory summary（注入 preamble）。
/// 遍历 tables/views/pipelines 目录，摘要每张表的探索状态+字段释义+关联关系。
pub fn get_okf_memory_summary(ws_path: &str) -> String {
    // 从 table_registry 读取已注册的表（结构化元数据）。
    let entries = crate::db::list_table_registry(ws_path).unwrap_or_default();
    let mut summary = String::new();

    if !entries.is_empty() {
        let mut blocks = Vec::new();
        for e in &entries {
            let icon = match e.status.as_str() {
                "available" => "✅",
                "unavailable_permanent" => "❌",
                "unavailable_temporary" => "⚠️",
                _ => "❓",
            };
            let mode = if e.access_mode == "pushdown" { " [pushdown]" } else { "" };
            let reason = if e.status != "available" && e.unavailable_reason.is_some() {
                format!(" — 不可用: {}", e.unavailable_reason.as_ref().unwrap())
            } else { String::new() };
            // 从 OKF 读取字段释义和关联关系（业务知识仍在 OKF）。
            let (col_comments, relations) = {
                let (_, c, r) = parse_column_semantics(ws_path, &e.local_name);
                (c, r)
            };
            let mut block = format!("- {icon} `{}` ({}){}{}", e.local_name, e.connection_name, mode, reason);
            if !col_comments.is_empty() {
                let cols: Vec<String> = col_comments.iter().map(|(k, v)| format!("`{k}`: {v}")).collect();
                block.push_str(&format!("\n  字段释义: {}", cols.join("; ")));
            }
            if !relations.is_empty() {
                block.push_str(&format!("\n  关联: {}", relations.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join(" ")));
            }
            blocks.push(block);
        }
        summary.push_str("# 工作区数据记忆\n以下是你之前探索过的表和知识，直接继承使用，无需重复探索：\n\n");
        summary.push_str(&blocks.join("\n"));
        summary.push_str("\n\n");
    }

    // pipelines 排障记录仍从 OKF 读。
    let okf_dir = get_okf_dir(ws_path);
    let pipes_dir = okf_dir.join("pipelines").join("specific");
    if pipes_dir.exists() {
        let mut entries: Vec<_> = Vec::new();
        if let Ok(dir) = fs::read_dir(&pipes_dir) {
            for e in dir.flatten() {
                if e.path().extension().and_then(|x| x.to_str()) == Some("md") {
                    entries.push(e);
                }
            }
        }
        if !entries.is_empty() {
            summary.push_str("# 排障记录\n");
            for e in &entries {
                let name = e.path().file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                summary.push_str(&format!("- {name}\n"));
            }
            summary.push_str("\n");
        }
    }

    summary.trim().to_string()
}

/// 更新表 OKF 文件的 frontmatter 状态字段。
/// 由 register_table / execute_query / sample_data 调用。
pub fn update_table_status(
    ws_path: &str,
    table_name: &str,
    status: &str,
    reason: Option<&str>,
) {
    let okf_dir = get_okf_dir(ws_path);
    let file_path = okf_dir.join("tables").join(format!("{table_name}.md"));
    if !file_path.exists() {
        // 文件不存在，创建一个带状态的骨架。
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            // 不应到这，但如果到就更新。
            let _ = content;
        }
        let body = format!(
            "---\ntype: DuckDB Table\ntitle: {table_name}\nstatus: {status}\nunavailable_reason: {}\nlast_explored: {}\n---\n\n# 字段 Schema\n（待探索）\n",
            reason.unwrap_or(""),
            current_timestamp()
        );
        let _ = std::fs::create_dir_all(file_path.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(&file_path, body);
        run_git_commit(&okf_dir, &file_path, &format!("Update status: tables/{table_name}"));
        return;
    }

    // 文件存在：更新 frontmatter 里的 status / unavailable_reason / last_explored。
    let content = match std::fs::read_to_string(&file_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut lines: Vec<String> = Vec::new();
    let mut in_yaml = false;
    let mut yaml_end = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            if !yaml_end {
                if in_yaml {
                    yaml_end = true;
                    // 在 YAML 关闭前插入/更新状态字段。
                    lines.push(format!("status: {status}"));
                    lines.push(format!("unavailable_reason: {}", reason.unwrap_or("")));
                    lines.push(format!("last_explored: {}", current_timestamp()));
                    in_yaml = false;
                } else {
                    in_yaml = true;
                }
            }
            lines.push(line.to_string());
            continue;
        }
        if in_yaml {
            // 跳过已有的 status / unavailable_reason / last_explored（用新值替代）。
            let lower = trimmed.to_lowercase();
            if lower.starts_with("status:") || lower.starts_with("unavailable_reason:") || lower.starts_with("last_explored:") {
                continue;
            }
        }
        lines.push(line.to_string());
    }
    let new_content = lines.join("\n");
    let _ = std::fs::write(&file_path, new_content);
    run_git_commit(&okf_dir, &file_path, &format!("Update status: tables/{table_name}"));
}
