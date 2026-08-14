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
        "# 物理画像\n- 行数估算: {row_count_str}\n\n# 字段 Schema\n{schema_table}\n# 关联关系\n- 暂无关联表（请手动编辑，例如 `- customer_id 关联 customers 表的 customer_id`）。\n"
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
    let body = format!("# 视图 SQL 定义\n```sql\n{sql}\n```\n");
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
    (title, col_comments, relations)
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
}
