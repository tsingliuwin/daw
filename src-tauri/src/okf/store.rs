//! OKF 文件存储：读/写/删/骨架/语义。基于 `OkfPaths` + `Versioner` + `Clock`。

use std::fs;
use std::path::{Path, PathBuf};

use crate::okf::frontmatter::{self, Frontmatter};
use crate::okf::markdown;
use crate::okf::model::{Category, ColumnInfo, ColumnSemantics, OkfReadOutcome, OkfWriteOutcome, Scope};
use crate::okf::paths::OkfPaths;
use crate::okf::Versioner;

// ---------- 路径辅助 ----------

fn category_dir(paths: &OkfPaths, ws: &str, category: Category) -> PathBuf {
    paths.category_dir(category.scope(), ws, category)
}

fn doc_file(paths: &OkfPaths, ws: &str, category: Category, name: &str) -> PathBuf {
    category_dir(paths, ws, category).join(format!("{name}.md"))
}

/// 版本提交的仓库根（全局类别→全局 okf 目录；工作区类别→工作区 okf 目录）。
fn repo_root(paths: &OkfPaths, ws: &str, scope: Scope) -> PathBuf {
    match scope {
        Scope::Global => paths.global_okf_dir(),
        Scope::Workspace => paths.workspace_okf_dir(ws),
    }
}

// ---------- 初始化（仅建目录，不再 seed index.md） ----------

pub fn init_global(paths: &OkfPaths) -> Result<(), String> {
    let g = paths.global_okf_dir();
    for sub in ["concepts", "users"] {
        fs::create_dir_all(g.join(sub)).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    Ok(())
}

pub fn init_workspace(paths: &OkfPaths, ws: &str) -> Result<(), String> {
    let w = paths.workspace_okf_dir(ws);
    for sub in ["tables", "views", "sources", "concepts", "pipelines/specific"] {
        fs::create_dir_all(w.join(sub)).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    Ok(())
}

// ---------- 读 ----------

pub fn read(
    paths: &OkfPaths,
    ws: &str,
    category: Category,
    name: &str,
    heading: &str,
) -> Result<OkfReadOutcome, String> {
    let file = doc_file(paths, ws, category, name);
    if !file.exists() {
        return Err(format!("文件不存在: {}", file.display()));
    }
    let content = fs::read_to_string(&file).map_err(|e| format!("读取文件失败: {e}"))?;
    let body = if heading.eq_ignore_ascii_case("all") || heading.trim().is_empty() {
        content
    } else {
        markdown::extract_block(&content, heading)
            .ok_or_else(|| format!("未找到标题为 '{heading}' 的板块"))?
    };
    Ok(OkfReadOutcome {
        scope: category.scope(),
        file_path: file,
        content: body,
    })
}

// ---------- 写 ----------

#[allow(clippy::too_many_arguments)]
pub fn write(
    paths: &OkfPaths,
    versioner: &dyn Versioner,
    clock: &dyn crate::okf::Clock,
    ws: &str,
    category: Category,
    name: &str,
    heading: &str,
    new_content: &str,
    description: Option<&str>,
) -> Result<OkfWriteOutcome, String> {
    let dir = category_dir(paths, ws, category);
    fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let file = dir.join(format!("{name}.md"));
    let ts = clock.now_ts();
    let created = !file.exists();

    let (fm, body) = if created {
        let mut f = Frontmatter::new();
        f.set("type", category.doc_type());
        f.set("title", name);
        f.set("description", description.unwrap_or(""));
        f.set("created_at", &ts);
        f.set("updated_at", &ts);
        (f, String::new())
    } else {
        let raw = fs::read_to_string(&file).map_err(|e| format!("读取文件失败: {e}"))?;
        let (fm_opt, body) = frontmatter::split_document(&raw);
        let mut f = fm_opt.unwrap_or_default();
        f.set("updated_at", &ts);
        if let Some(desc) = description {
            f.set("description", desc);
        }
        (f, body)
    };

    let level = if category == Category::Concept { 2 } else { 1 };
    let body = markdown::upsert_block(&body, heading, new_content, level);
    let body = markdown::deduplicate(&body);

    let content = frontmatter::join_document(Some(&fm), &body);
    fs::write(&file, content).map_err(|e| format!("写入文件失败: {e}"))?;
    let scope = category.scope();
    versioner.commit(
        &repo_root(paths, ws, scope),
        &file,
        &format!("Update OKF: {}/{name}", category.dir()),
    );

    Ok(OkfWriteOutcome {
        scope,
        file_path: file,
        created,
    })
}

// ---------- 删 ----------

/// 在工作区 tables/views/sources 下查找并删除 `<name>.md`，返回是否删到。
pub fn delete(paths: &OkfPaths, versioner: &dyn Versioner, ws: &str, name: &str) -> Result<bool, String> {
    let w = paths.workspace_okf_dir(ws);
    let mut any = false;
    for cat in [Category::Table, Category::View, Category::Source] {
        let f = w.join(cat.dir()).join(format!("{name}.md"));
        if f.exists() {
            let _ = fs::remove_file(&f);
            versioner.commit(&w, &f, &format!("Delete OKF: {name}"));
            any = true;
        }
    }
    Ok(any)
}

// ---------- 骨架 ----------

/// 首次探索表时生成 `tables/<table>.md` 骨架；已存在不覆盖（保留业务释义）。
/// 返回是否新建。
pub fn ensure_table_skeleton(
    paths: &OkfPaths,
    versioner: &dyn Versioner,
    clock: &dyn crate::okf::Clock,
    ws: &str,
    table: &str,
    columns: &[ColumnInfo],
    row_count: Option<i64>,
) -> Result<bool, String> {
    let file = doc_file(paths, ws, Category::Table, table);
    if file.exists() {
        return Ok(false);
    }
    fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
        .map_err(|e| format!("创建目录失败: {e}"))?;
    let ts = clock.now_ts();
    let mut fm = Frontmatter::new();
    fm.set("type", Category::Table.doc_type());
    fm.set("title", &format!("{table} 物理数据表"));
    fm.set("created_at", &ts);
    fm.set("updated_at", &ts);

    let mut schema_table = String::from("| 字段名 | 物理类型 | 业务释义 | 数据约束 |\n|---|---|---|---|\n");
    for (name, ty, nullable) in columns {
        let constraint = if *nullable { "" } else { "NOT NULL" };
        schema_table.push_str(&format!("| `{name}` | {ty} |  | {constraint} |\n"));
    }
    let row_count_str = row_count.map(|c| c.to_string()).unwrap_or_else(|| "未知".to_string());
    let body = format!(
        "# 物理画像\n- 行数估算: {row_count_str}\n\n# 字段 Schema\n{schema_table}\n# 关联关系\n（暂无。格式：`- \\`本地列\\` → \\`[[目标表]]\\`.\\`目标列\\` (N:1) 描述`）\n\n# 业务描述\n（待补充）\n"
    );
    let content = frontmatter::join_document(Some(&fm), &body);
    fs::write(&file, content).map_err(|e| format!("写入失败: {e}"))?;
    versioner.commit(
        &paths.workspace_okf_dir(ws),
        &file,
        &format!("Bootstrap table OKF: {table}"),
    );
    Ok(true)
}

/// 创建视图时生成 `views/<view>.md` 骨架；已存在不覆盖。返回是否新建。
pub fn ensure_view_skeleton(
    paths: &OkfPaths,
    versioner: &dyn Versioner,
    clock: &dyn crate::okf::Clock,
    ws: &str,
    view: &str,
    sql: &str,
) -> Result<bool, String> {
    let file = doc_file(paths, ws, Category::View, view);
    if file.exists() {
        return Ok(false);
    }
    fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
        .map_err(|e| format!("创建目录失败: {e}"))?;
    let ts = clock.now_ts();
    let mut fm = Frontmatter::new();
    fm.set("type", Category::View.doc_type());
    fm.set("title", &format!("{view} 逻辑视图"));
    fm.set("created_at", &ts);
    fm.set("updated_at", &ts);
    let body = format!(
        "# 视图 SQL 定义\n```sql\n{sql}\n```\n\n# 字段释义\n| 字段名 | 业务释义 |\n|---|---|\n（待补充）\n\n# 依赖关系\n（暂无。格式：`- [[表/视图名]] (N:1) 描述`）\n\n# 业务描述\n（待补充）\n"
    );
    let content = frontmatter::join_document(Some(&fm), &body);
    fs::write(&file, content).map_err(|e| format!("写入失败: {e}"))?;
    versioner.commit(
        &paths.workspace_okf_dir(ws),
        &file,
        &format!("Bootstrap view OKF: {view}"),
    );
    Ok(true)
}

// ---------- 语义解析 ----------

/// 解析表/视图 OKF：(业务标题, {列名→释义}, 关联关系)。
/// 查找顺序 tables → views → sources。
pub fn column_semantics(paths: &OkfPaths, ws: &str, name: &str) -> ColumnSemantics {
    let w = paths.workspace_okf_dir(ws);
    let mut file = w.join("tables").join(format!("{name}.md"));
    if !file.exists() {
        file = w.join("views").join(format!("{name}.md"));
    }
    if !file.exists() {
        file = w.join("sources").join(format!("{name}.md"));
    }
    if !file.exists() {
        return (None, std::collections::HashMap::new(), Vec::new());
    }
    let Ok(content) = fs::read_to_string(&file) else {
        return (None, std::collections::HashMap::new(), Vec::new());
    };
    parse_semantics_from_content(&content)
}

/// 从 OKF 文件全文解析列语义（纯函数，便于测试）。
pub fn parse_semantics_from_content(content: &str) -> ColumnSemantics {
    let (fm, _body) = frontmatter::split_document(content);
    let title = fm.and_then(|f| f.get("title").map(|s| s.to_string()));

    let mut col_comments: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut relations = Vec::new();
    let mut current_heading = "";
    for line in content.lines() {
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
        } else if (current_heading == "关联关系" || current_heading == "Relationships")
            && (trimmed.starts_with('-') || trimmed.starts_with('*'))
        {
            let rel = trimmed.trim_start_matches(['-', '*', ' ']).to_string();
            if !rel.is_empty() {
                relations.push(rel);
            }
        }
    }
    (title, col_comments, relations)
}

/// 读取表/视图 OKF 并用新版 parser 解析（结构化 TableSemantics，含关联内链）。
#[allow(dead_code)]
pub fn table_semantics(paths: &OkfPaths, ws: &str, name: &str) -> crate::okf::model::TableSemantics {
    let w = paths.workspace_okf_dir(ws);
    let mut file = w.join("tables").join(format!("{name}.md"));
    if !file.exists() {
        file = w.join("views").join(format!("{name}.md"));
    }
    if !file.exists() {
        file = w.join("sources").join(format!("{name}.md"));
    }
    if !file.exists() {
        return crate::okf::model::TableSemantics::default();
    }
    match fs::read_to_string(&file) {
        Ok(content) => parse_table_semantics(&content),
        Err(_) => crate::okf::model::TableSemantics::default(),
    }
}

// ---------- 元数据（frontmatter）读写 ----------

/// 读取一个 OKF 文件的元数据（结构化 Frontmatter）。文件不存在则报错。
pub fn read_metadata(paths: &OkfPaths, ws: &str, category: Category, name: &str) -> Result<Frontmatter, String> {
    let file = doc_file(paths, ws, category, name);
    if !file.exists() {
        return Err(format!("文件不存在: {}", file.display()));
    }
    let content = fs::read_to_string(&file).map_err(|e| format!("读取文件失败: {e}"))?;
    let (fm, _) = frontmatter::split_document(&content);
    Ok(fm.unwrap_or_default())
}

/// 更新元数据：只改 frontmatter 指定字段，正文不动，自动刷 updated_at。
/// 文件不存在则报错。
pub fn update_metadata(
    paths: &OkfPaths,
    versioner: &dyn Versioner,
    clock: &dyn crate::okf::Clock,
    ws: &str,
    category: Category,
    name: &str,
    fields: &[(String, String)],
) -> Result<(), String> {
    let file = doc_file(paths, ws, category, name);
    if !file.exists() {
        return Err(format!("文件不存在: {}", file.display()));
    }
    let content = fs::read_to_string(&file).map_err(|e| format!("读取文件失败: {e}"))?;
    let (fm_opt, body) = frontmatter::split_document(&content);
    let mut fm = fm_opt.unwrap_or_default();
    for (k, v) in fields {
        fm.set(k, v);
    }
    fm.set("updated_at", &clock.now_ts());
    let new_content = frontmatter::join_document(Some(&fm), &body);
    fs::write(&file, new_content).map_err(|e| format!("写入文件失败: {e}"))?;
    versioner.commit(
        &repo_root(paths, ws, category.scope()),
        &file,
        &format!("Update metadata: {}/{name}", category.dir()),
    );
    Ok(())
}

// ---------- 删除 / 重命名（知识整理原语） ----------

/// 删除一条知识文件（任意类别：concepts/users 全局，其余工作区）。
/// `merge_into=Some(target)` 时先把全库 `[[name]]` 内链改写为 `[[target]]`
/// （合并去重场景：内容已写入保留文件后再删冗余）。返回是否删到。
pub fn delete_doc(
    paths: &OkfPaths,
    versioner: &dyn Versioner,
    ws: &str,
    category: Category,
    name: &str,
    merge_into: Option<&str>,
) -> Result<bool, String> {
    let file = doc_file(paths, ws, category, name);
    if !file.exists() {
        return Ok(false);
    }
    if let Some(target) = merge_into {
        rewrite_wikilinks(paths, versioner, ws, name, target);
    }
    fs::remove_file(&file).map_err(|e| format!("删除失败: {e}"))?;
    versioner.commit(
        &repo_root(paths, ws, category.scope()),
        &file,
        &format!("Delete OKF: {}/{}", category.dir(), name),
    );
    Ok(true)
}

/// 重命名知识文件：移动 + 改 frontmatter title + 刷 updated_at +
/// 全库内链 `[[old]]`→`[[new]]`。返回新路径。
pub fn rename_doc(
    paths: &OkfPaths,
    versioner: &dyn Versioner,
    clock: &dyn crate::okf::Clock,
    ws: &str,
    category: Category,
    old: &str,
    new: &str,
) -> Result<PathBuf, String> {
    if old == new {
        return Err("新旧名称相同".to_string());
    }
    let src = doc_file(paths, ws, category, old);
    if !src.exists() {
        return Err(format!("文件不存在: {}", src.display()));
    }
    let dst = doc_file(paths, ws, category, new);
    if dst.exists() {
        return Err(format!("目标已存在: {}", dst.display()));
    }
    fs::rename(&src, &dst).map_err(|e| format!("重命名失败: {e}"))?;
    let content = fs::read_to_string(&dst).map_err(|e| format!("读取文件失败: {e}"))?;
    let (fm_opt, body) = frontmatter::split_document(&content);
    let mut fm = fm_opt.unwrap_or_default();
    fm.set("title", new);
    fm.set("updated_at", &clock.now_ts());
    fs::write(&dst, frontmatter::join_document(Some(&fm), &body))
        .map_err(|e| format!("写入失败: {e}"))?;
    rewrite_wikilinks(paths, versioner, ws, old, new);
    versioner.commit(
        &repo_root(paths, ws, category.scope()),
        &dst,
        &format!("Rename OKF: {}/{} -> {}", category.dir(), old, new),
    );
    Ok(dst)
}

/// 全库（全局 + 当前工作区）把 `[[old]]` 内链改写为 `[[new]]` 并提交。
/// 只处理精确 `[[old]]` 形态（本库内链语法）。返回改写的文件数。
fn rewrite_wikilinks(paths: &OkfPaths, versioner: &dyn Versioner, ws: &str, old: &str, new: &str) -> usize {
    let from = format!("[[{old}]]");
    let to = format!("[[{new}]]");
    let mut changed = 0;
    for root in [paths.global_okf_dir(), paths.workspace_okf_dir(ws)] {
        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_entry(|e| e.file_name().to_string_lossy() != ".git")
            .flatten()
        {
            let p = entry.path();
            if !p.is_file() || p.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(content) = fs::read_to_string(p) else { continue };
            if !content.contains(&from) {
                continue;
            }
            if fs::write(p, content.replace(&from, &to)).is_ok() {
                versioner.commit(&root, p, &format!("Rewrite links: {old} -> {new}"));
                changed += 1;
            }
        }
    }
    changed
}

// ===========================================================================
// 新版 parser（step3）：结构化关联内链 + 表头列定位 + 大小写不敏感
// ===========================================================================

#[allow(dead_code)]
fn extract_backtick(s: &str) -> Option<String> {
    let start = s.find('`')?;
    let rest = &s[start + 1..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

#[allow(dead_code)]
fn extract_wiki_link(s: &str) -> Option<String> {
    let start = s.find("[[")?;
    let rest = &s[start + 2..];
    let end = rest.find("]]")?;
    Some(rest[..end].to_string())
}

/// 找方向箭头 (Direction, 起始 byte, 结束 byte)。优先 UTF-8 箭头。
#[allow(dead_code)]
fn find_arrow(s: &str) -> Option<(crate::okf::model::Direction, usize, usize)> {
    use crate::okf::model::Direction;
    for (arrow, dir) in [
        ("→", Direction::OneWay),
        ("↔", Direction::TwoWay),
        ("<->", Direction::TwoWay),
        ("->", Direction::OneWay),
    ] {
        if let Some(pos) = s.find(arrow) {
            return Some((dir, pos, pos + arrow.len()));
        }
    }
    None
}

#[allow(dead_code)]
fn extract_paren_cardinality(s: &str) -> Option<(crate::okf::model::Cardinality, usize)> {
    use crate::okf::model::Cardinality;
    let start = s.find('(')?;
    let rest = &s[start + 1..];
    let end = rest.find(')')?;
    let card = Cardinality::from_str(rest[..end].trim())?;
    Some((card, start + 1 + end + 1))
}

/// 解析一行结构化关联/依赖关系。
/// 表格式：`- \`local_col\` → [[target_table]].\`target_col\` (N:1) 描述`
/// 视图依赖格式：`- [[target_table]] (N:1) 描述`
#[allow(dead_code)]
fn parse_relation_line(line: &str) -> Option<crate::okf::model::Relation> {
    use crate::okf::model::{Direction, Relation};
    let s = line.trim().trim_start_matches(['-', '*', ' ']).trim();
    if s.is_empty() {
        return None;
    }
    // 完整表格式：有方向箭头
    if let Some((direction, arrow_start, arrow_end)) = find_arrow(s) {
        let local_col = extract_backtick(&s[..arrow_start])?;
        let right = s[arrow_end..].trim();
        let target_table = extract_wiki_link(right)?;
        let wiki_end = right.find("]]")? + 2;
        let target_col = extract_backtick(&right[wiki_end..]).unwrap_or_default();
        let (cardinality, paren_end) = extract_paren_cardinality(right)?;
        let desc = right[paren_end..].trim();
        return Some(Relation {
            local_col,
            direction,
            target_table,
            target_col,
            cardinality,
            description: if desc.is_empty() { None } else { Some(desc.to_string()) },
        });
    }
    // 简单视图依赖格式：[[target]] (card) desc
    if let Some(target_table) = extract_wiki_link(s) {
        if let Some((cardinality, paren_end)) = extract_paren_cardinality(s) {
            let desc = s[paren_end..].trim();
            return Some(Relation {
                local_col: String::new(),
                direction: Direction::OneWay,
                target_table,
                target_col: String::new(),
                cardinality,
                description: if desc.is_empty() { None } else { Some(desc.to_string()) },
            });
        }
    }
    None
}

/// 从 markdown 表格行提取 cells（去首尾空管道产生的空串）。
#[allow(dead_code)]
fn table_cells(line: &str) -> Vec<String> {
    line.split('|')
        .map(|c| c.trim().to_string())
        .collect::<Vec<_>>()
}

#[allow(dead_code)]
fn is_separator_row(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|c| c.is_empty() || c.chars().all(|ch| ch == '-'))
}

/// 解析表/视图 OKF：frontmatter title + 字段表（表头列定位）+ 关联关系（结构化内链）。
/// 大小写不敏感匹配标题；列索引从表头行动态定位（不再硬编码 parts[1]/parts[3]）。
#[allow(dead_code)]
pub fn parse_table_semantics(content: &str) -> crate::okf::model::TableSemantics {
    use crate::okf::model::{ColumnSemantic, TableSemantics};
    let (fm, _body) = frontmatter::split_document(content);
    let title = fm.and_then(|f| f.get("title").map(|s| s.to_string()));

    let mut columns = Vec::new();
    let mut relations = Vec::new();
    let mut current = "";
    let mut header_map: Option<std::collections::HashMap<&str, usize>> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            current = trimmed.trim_start_matches('#').trim();
            header_map = None;
            continue;
        }
        // 字段 Schema / 字段释义 板块（大小写不敏感）
        if current.eq_ignore_ascii_case("字段 Schema")
            || current.eq_ignore_ascii_case("Column Schema")
            || current.eq_ignore_ascii_case("字段释义")
        {
            if !trimmed.starts_with('|') {
                continue;
            }
            let cells = table_cells(trimmed);
            // 去掉 split 在首尾 | 产生的空串
            let cells: Vec<String> = cells
                .iter()
                .skip_while(|c| c.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .iter()
                .rev()
                .skip_while(|c| c.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if is_separator_row(&cells) {
                continue;
            }
            if header_map.is_none() {
                let mut idx = std::collections::HashMap::new();
                for (i, cell) in cells.iter().enumerate() {
                    let lower = cell.to_lowercase();
                    if lower.contains("字段名") || lower.contains("column") || lower.contains("field") || lower.contains("列名") {
                        idx.insert("name", i);
                    }
                    if lower.contains("物理类型") || lower.contains("type") || lower.contains("类型") {
                        idx.insert("ty", i);
                    }
                    if lower.contains("业务释义") || lower.contains("comment") || lower.contains("释义") {
                        idx.insert("comment", i);
                    }
                    if lower.contains("数据约束") || lower.contains("constraint") || lower.contains("约束") {
                        idx.insert("constraint", i);
                    }
                }
                if !idx.contains_key("name") && !cells.is_empty() {
                    idx.insert("name", 0);
                }
                if !idx.contains_key("comment") && cells.len() >= 2 {
                    idx.insert("comment", 1);
                }
                header_map = Some(idx);
                continue;
            }
            let idx = header_map.as_ref().unwrap();
            let get = |key: &str| -> String {
                idx.get(key)
                    .and_then(|&i| cells.get(i))
                    .map(|s| s.trim_matches('`').trim().to_string())
                    .unwrap_or_default()
            };
            let name = get("name");
            if !name.is_empty() {
                columns.push(ColumnSemantic {
                    name,
                    ty: get("ty"),
                    comment: get("comment"),
                    constraint: get("constraint"),
                });
            }
        }
        // 关联关系 / 依赖关系 板块（大小写不敏感）
        else if (current.eq_ignore_ascii_case("关联关系")
            || current.eq_ignore_ascii_case("Relationships")
            || current.eq_ignore_ascii_case("依赖关系")
            || current.eq_ignore_ascii_case("Dependencies"))
            && (trimmed.starts_with('-') || trimmed.starts_with('*'))
        {
            if let Some(rel) = parse_relation_line(trimmed) {
                relations.push(rel);
            }
        }
    }

    TableSemantics { title, columns, relations }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okf::{Clock, NoopVersioner};
    use std::sync::Arc;

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_ts(&self) -> String {
            "2026-08-14T00:00:00Z".to_string()
        }
    }

    fn okf(root: &Path) -> (OkfPaths, Arc<dyn Versioner>, Arc<dyn Clock>) {
        (
            OkfPaths::new(root.to_path_buf()),
            Arc::new(NoopVersioner),
            Arc::new(FixedClock),
        )
    }

    #[test]
    fn write_creates_then_updates() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        // 首建
        let o1 = write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "co", "业务描述", "first", None).unwrap();
        assert!(o1.created);
        let raw = fs::read_to_string(&o1.file_path).unwrap();
        assert!(raw.contains("type: Business Concept"));
        assert!(raw.contains("created_at: 2026-08-14"));
        assert!(raw.contains("updated_at: 2026-08-14"));
        assert!(raw.contains("## 业务描述"));
        assert!(raw.contains("first"));
        // 更新同板块
        let o2 = write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "co", "业务描述", "second", None).unwrap();
        assert!(!o2.created);
        let raw2 = fs::read_to_string(&o2.file_path).unwrap();
        assert!(raw2.contains("second"));
        assert!(!raw2.contains("first"));
    }

    #[test]
    fn write_updates_description_when_provided() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "c", "h", "body", Some("desc1")).unwrap();
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "c", "h", "body2", Some("desc2")).unwrap();
        let f = doc_file(&paths, "ws", Category::Concept, "c");
        let raw = fs::read_to_string(&f).unwrap();
        assert!(raw.contains("description: desc2"));
        assert!(!raw.contains("desc1"));
    }

    #[test]
    fn write_preserves_created_at_on_update() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "c", "h", "b", None).unwrap();
        // 模拟时间变化：再写一次，created_at 应保留为 FixedClock 的同一值（这里时钟固定）。
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "c", "h2", "b2", None).unwrap();
        let f = doc_file(&paths, "ws", Category::Concept, "c");
        let raw = fs::read_to_string(&f).unwrap();
        // 两个标题都在，created_at 只有一个
        assert_eq!(raw.matches("created_at:").count(), 1);
    }

    #[test]
    fn read_all_returns_whole_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "c", "业务描述", "content", None).unwrap();
        let o = read(&paths, "ws", Category::Concept, "c", "all").unwrap();
        assert!(o.content.contains("type: Business Concept"));
        assert!(o.content.contains("业务描述"));
    }

    #[test]
    fn read_specific_heading() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "c", "业务描述", "the-desc", None).unwrap();
        let o = read(&paths, "ws", Category::Concept, "c", "业务描述").unwrap();
        assert_eq!(o.content, "the-desc");
    }

    #[test]
    fn read_missing_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, _ver, _clock) = okf(tmp.path());
        let err = read(&paths, "ws", Category::Concept, "nope", "all").unwrap_err();
        assert!(err.contains("文件不存在"));
    }

    #[test]
    fn ensure_table_skeleton_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        let cols = vec![("id".to_string(), "BIGINT".to_string(), false), ("name".to_string(), "VARCHAR".to_string(), true)];
        let created1 = ensure_table_skeleton(&paths, ver.as_ref(), clock.as_ref(), "ws", "t", &cols, Some(100)).unwrap();
        assert!(created1);
        // 第二次不覆盖
        let created2 = ensure_table_skeleton(&paths, ver.as_ref(), clock.as_ref(), "ws", "t", &cols, Some(200)).unwrap();
        assert!(!created2);
        let f = doc_file(&paths, "ws", Category::Table, "t");
        let raw = fs::read_to_string(&f).unwrap();
        assert!(raw.contains("行数估算: 100")); // 未被 200 覆盖
        assert!(raw.contains("`id`"));
    }

    #[test]
    fn delete_removes_table_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        let cols = vec![];
        ensure_table_skeleton(&paths, ver.as_ref(), clock.as_ref(), "ws", "t", &cols, None).unwrap();
        let any = delete(&paths, ver.as_ref(), "ws", "t").unwrap();
        assert!(any);
        let f = doc_file(&paths, "ws", Category::Table, "t");
        assert!(!f.exists());
    }

    #[test]
    fn parse_semantics_extracts_title_columns_relations() {
        let doc = "---\ntype: DuckDB Table\ntitle: 订单表\n---\n\n# 字段 Schema\n| 字段名 | 物理类型 | 业务释义 | 数据约束 |\n|---|---|---|---|\n| `order_id` | BIGINT | 订单编号 | NOT NULL |\n\n# 关联关系\n- order_id 关联 users.user_id\n";
        let (title, cols, rels) = parse_semantics_from_content(doc);
        assert_eq!(title.as_deref(), Some("订单表"));
        assert_eq!(cols.get("order_id").map(|s| s.as_str()), Some("订单编号"));
        assert!(rels.iter().any(|r| r.contains("users.user_id")));
    }

    #[test]
    fn parse_relation_structured_one_way() {
        use crate::okf::model::{Cardinality, Direction};
        let line = "- `customer_id` → [[customers]].`customer_id` (N:1) 客户关联";
        let rel = parse_relation_line(line).unwrap();
        assert_eq!(rel.local_col, "customer_id");
        assert_eq!(rel.direction, Direction::OneWay);
        assert_eq!(rel.target_table, "customers");
        assert_eq!(rel.target_col, "customer_id");
        assert_eq!(rel.cardinality, Cardinality::ManyToOne);
        assert_eq!(rel.description.as_deref(), Some("客户关联"));
    }

    #[test]
    fn parse_relation_structured_two_way_many_to_many() {
        use crate::okf::model::{Cardinality, Direction};
        let line = "- `tag_id` ↔ [[order_tags]].`order_id` (N:M) 多对多中间表";
        let rel = parse_relation_line(line).unwrap();
        assert_eq!(rel.direction, Direction::TwoWay);
        assert_eq!(rel.cardinality, Cardinality::ManyToMany);
        assert_eq!(rel.target_table, "order_tags");
        assert_eq!(rel.description.as_deref(), Some("多对多中间表"));
    }

    #[test]
    fn parse_relation_no_description() {
        let line = "- `product_id` → [[products]].`product_id` (N:1)";
        let rel = parse_relation_line(line).unwrap();
        assert!(rel.description.is_none());
    }

    #[test]
    fn parse_relation_non_structured_returns_none() {
        let line = "- 暂无关联表";
        assert!(parse_relation_line(line).is_none());
    }

    #[test]
    fn parse_table_semantics_case_insensitive_and_header_located() {
        let doc = "---\ntype: DuckDB Table\ntitle: 订单表\n---\n\n# 字段 schema\n| 列名 | 类型 | 释义 | 约束 |\n|---|---|---|---|\n| `amount` | DECIMAL | 金额 | NOT NULL |\n\n# relationships\n- `customer_id` → [[customers]].`customer_id` (N:1) 客户\n";
        let ts = parse_table_semantics(doc);
        assert_eq!(ts.title.as_deref(), Some("订单表"));
        assert_eq!(ts.columns.len(), 1);
        assert_eq!(ts.columns[0].name, "amount");
        assert_eq!(ts.columns[0].ty, "DECIMAL");
        assert_eq!(ts.columns[0].comment, "金额");
        assert_eq!(ts.columns[0].constraint, "NOT NULL");
        assert_eq!(ts.relations.len(), 1);
        assert_eq!(ts.relations[0].target_table, "customers");
    }

    #[test]
    fn parse_table_semantics_view_field_table() {
        // 视图的字段释义表（两列：字段名|业务释义）
        let doc = "---\ntype: DuckDB View\ntitle: v_summary\n---\n\n# 字段释义\n| 字段名 | 业务释义 |\n|---|---|\n| `total` | 总计 |\n\n# 依赖关系\n- [[orders]] (N:1) 订单主表\n";
        let ts = parse_table_semantics(doc);
        assert_eq!(ts.columns.len(), 1);
        assert_eq!(ts.columns[0].name, "total");
        assert_eq!(ts.columns[0].comment, "总计");
        assert_eq!(ts.relations.len(), 1);
        assert_eq!(ts.relations[0].target_table, "orders");
    }

    #[test]
    fn read_metadata_returns_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "co", "业务描述", "body", Some("a desc")).unwrap();
        let fm = read_metadata(&paths, "ws", Category::Concept, "co").unwrap();
        assert_eq!(fm.get("type"), Some("Business Concept"));
        assert_eq!(fm.get("description"), Some("a desc"));
        assert!(fm.get("created_at").is_some());
    }

    #[test]
    fn read_metadata_missing_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, _ver, _clock) = okf(tmp.path());
        assert!(read_metadata(&paths, "ws", Category::Concept, "nope").is_err());
    }

    #[test]
    fn update_metadata_changes_fields_without_touching_body() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "co", "业务描述", "the-body", Some("old desc")).unwrap();
        // 只改 title + description，不动正文
        let fields = vec![("title".to_string(), "新标题".to_string()), ("description".to_string(), "new desc".to_string())];
        update_metadata(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "co", &fields).unwrap();
        // 元数据变了
        let fm = read_metadata(&paths, "ws", Category::Concept, "co").unwrap();
        assert_eq!(fm.get("title"), Some("新标题"));
        assert_eq!(fm.get("description"), Some("new desc"));
        // 正文未被破坏
        let o = read(&paths, "ws", Category::Concept, "co", "业务描述").unwrap();
        assert_eq!(o.content, "the-body");
    }

    #[test]
    fn update_metadata_bumps_updated_at() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "co", "h", "b", None).unwrap();
        let before = read_metadata(&paths, "ws", Category::Concept, "co").unwrap();
        let created = before.get("created_at").map(|s| s.to_string());
        update_metadata(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "co", &[("description".into(), "d".into())]).unwrap();
        let after = read_metadata(&paths, "ws", Category::Concept, "co").unwrap();
        // created_at 不变
        assert_eq!(after.get("created_at"), created.as_deref());
        // updated_at 字段存在（FixedClock 固定值，只需确认字段在）
        assert!(after.get("updated_at").is_some());
    }

    #[test]
    fn delete_doc_concepts_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "co", "业务描述", "body", None).unwrap();
        // 删除全局 concept
        assert!(delete_doc(&paths, ver.as_ref(), "ws", Category::Concept, "co", None).unwrap());
        assert!(!doc_file(&paths, "ws", Category::Concept, "co").exists());
        // 不存在的文件返回 false 而非报错
        assert!(!delete_doc(&paths, ver.as_ref(), "ws", Category::Concept, "co", None).unwrap());
    }

    #[test]
    fn delete_doc_with_merge_rewrites_links() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        // 保留文件 + 冗余文件 + 引用冗余文件的表知识（内链）
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "org_consult_center", "组织架构", "权威内容", None).unwrap();
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "consult_center_org", "组织架构", "旧内容", None).unwrap();
        let cols = vec![("dept_id".to_string(), "BIGINT".to_string(), false)];
        ensure_table_skeleton(&paths, ver.as_ref(), clock.as_ref(), "ws", "v_dept", &cols, None).unwrap();
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Table, "v_dept", "关联关系",
            "- `dept_id` → [[consult_center_org]].`dept_id` (N:1) 部门维度", None).unwrap();

        // 删除冗余并指向保留文件
        assert!(delete_doc(&paths, ver.as_ref(), "ws", Category::Concept, "consult_center_org", Some("org_consult_center")).unwrap());
        // 内链已改写
        let table_raw = fs::read_to_string(doc_file(&paths, "ws", Category::Table, "v_dept")).unwrap();
        assert!(table_raw.contains("[[org_consult_center]]"));
        assert!(!table_raw.contains("[[consult_center_org]]"));
        // 冗余文件已删
        assert!(!doc_file(&paths, "ws", Category::Concept, "consult_center_org").exists());
    }

    #[test]
    fn rename_doc_moves_retitles_and_rewrites_links() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "old_name", "业务描述", "内容保留", Some("desc")).unwrap();
        let cols = vec![("x".to_string(), "BIGINT".to_string(), true)];
        ensure_table_skeleton(&paths, ver.as_ref(), clock.as_ref(), "ws", "t", &cols, None).unwrap();
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Table, "t", "关联关系", "- [[old_name]] (N:1) 概念引用", None).unwrap();

        let dst = rename_doc(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "old_name", "new_name").unwrap();
        // 文件已移动，frontmatter title 已改，正文保留
        let raw = fs::read_to_string(&dst).unwrap();
        assert!(raw.contains("title: new_name"));
        assert!(raw.contains("内容保留"));
        assert!(!doc_file(&paths, "ws", Category::Concept, "old_name").exists());
        // 引用方内链已同步改写
        let table_raw = fs::read_to_string(doc_file(&paths, "ws", Category::Table, "t")).unwrap();
        assert!(table_raw.contains("[[new_name]]"));
        assert!(!table_raw.contains("[[old_name]]"));
    }

    #[test]
    fn rename_doc_rejects_same_and_missing_and_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let (paths, ver, clock) = okf(tmp.path());
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "a", "h", "b", None).unwrap();
        write(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "b", "h", "b", None).unwrap();
        assert!(rename_doc(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "a", "a").is_err());
        assert!(rename_doc(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "nope", "x").is_err());
        assert!(rename_doc(&paths, ver.as_ref(), clock.as_ref(), "ws", Category::Concept, "a", "b").is_err());
    }
}
