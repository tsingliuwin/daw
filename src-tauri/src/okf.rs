//! OKF（Open Knowledge Format）— per-workspace 业务知识库。
//!
//! 把表/字段的业务释义、关联关系、排障经验结构化存成 Markdown，
//! 让 agent 按需读取（渐进式披露）而非全量塞进 prompt。
//!
//! 目录结构（`<workspace>/okf/`）：
//!   tables/       — 物理表 schema + 业务释义 + 关联关系
//!   views/        — 逻辑视图定义
//!   concepts/     — 全局业务概念（公司背景/名词/指标字典）
//!   pipelines/specific/ — 导入/清洗配方 + 排障记录
//!
//! 文件格式：YAML frontmatter + Markdown body。
//! describe_table 调 parse_column_semantics 注入列释义。
//! agent 调 write_okf_block 写入业务知识（自动 git commit）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Get the root OKF directory: `<ws_path>/okf`
pub fn get_okf_dir(ws_path: &str) -> PathBuf {
    Path::new(ws_path).join("okf")
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
/// category fallback: tables/ → views/ → sources/
pub fn read_okf_block(
    ws_path: &str,
    category: &str,
    name: &str,
    heading: &str,
) -> Result<String, String> {
    let okf_dir = get_okf_dir(ws_path);
    let requested_path = okf_dir.join(category).join(format!("{name}.md"));
    let mut file_path = requested_path.clone();
    if !file_path.exists() {
        let candidates = [
            okf_dir.join("tables").join(format!("{name}.md")),
            okf_dir.join("views").join(format!("{name}.md")),
            okf_dir.join("sources").join(format!("{name}.md")),
        ];
        let mut found = false;
        for c in &candidates {
            if c.exists() {
                file_path = c.clone();
                found = true;
                break;
            }
        }
        if !found {
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
    let okf_dir = ensure_okf_dirs(ws_path)?;
    // 写操作不跨类别 fallback——避免 concept 内容写进 table 文件造成数据污染。
    let file_path = okf_dir.join(category).join(format!("{name}.md"));

    let content = if file_path.exists() {
        fs::read_to_string(&file_path).map_err(|e| format!("读取文件失败: {e}"))?
    } else {
        let doc_type = match category {
            "tables" => "DuckDB Table",
            "views" => "DuckDB View",
            "sources" => "Data Source",
            "concepts" => "Business Concept",
            _ => "Concept",
        };
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
