//! OKF 目录大纲引擎：自动扫描生成 preamble summary + 全文搜索。
//!
//! 大纲的唯一来源——不依赖 index.md，每次实时扫描文件目录。
//! 表清单来自 table_registry（结构化，由调用方传入），其余类别扫描目录。

use std::fs;
use std::path::Path;

use crate::model::TableRegistryEntry;
use crate::okf::markdown;
use crate::okf::model::{Scope, SearchHit, TableStatus};
use crate::okf::paths::OkfPaths;
use crate::okf::store;

/// 生成注入 preamble 的大纲：表（状态+字段释义+关联）+ 全局概念 + 视图 + 数据源 + 排障。
pub fn summary(paths: &OkfPaths, ws: &str, table_entries: &[TableRegistryEntry]) -> String {
    let mut out = String::new();

    // 1. 工作区数据记忆（表，来自 table_registry）。
    if !table_entries.is_empty() {
        let mut blocks = Vec::new();
        for e in table_entries {
            let icon = TableStatus::from_str(&e.status).icon();
            let mode = if e.access_mode == "pushdown" { " [pushdown]" } else { "" };
            let reason = if e.status != "available" {
                e.unavailable_reason
                    .as_ref()
                    .map(|r| format!(" — 不可用: {r}"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let (_, col_comments, relations) = store::column_semantics(paths, ws, &e.local_name);
            let mut block = format!("- {icon} `{}` ({}){}{}", e.local_name, e.connection_name, mode, reason);
            if !col_comments.is_empty() {
                let cols: Vec<String> = col_comments.iter().map(|(k, v)| format!("`{k}`: {v}")).collect();
                block.push_str(&format!("\n  字段释义: {}", cols.join("; ")));
            }
            if !relations.is_empty() {
                block.push_str(&format!(
                    "\n  关联: {}",
                    relations.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join(" ")
                ));
            }
            blocks.push(block);
        }
        out.push_str("# 工作区数据记忆\n以下是你之前探索过的表和知识，直接继承使用，无需重复探索：\n\n");
        out.push_str(&blocks.join("\n"));
        out.push_str("\n\n");
    }

    // 2. 全局业务概念（名 + 二级标题索引）。
    let concepts = list_with_headings(&paths.global_okf_dir().join("concepts"), 2);
    if !concepts.is_empty() {
        out.push_str(
            "# 业务概念（全局）\n以下业务背景已沉淀，需要细节时用 load_okf_block(category=\"concepts\", name=\"<名>\", heading=\"<标题>\") 读取：\n",
        );
        for (name, headings) in &concepts {
            if headings.is_empty() {
                out.push_str(&format!("- {name}\n"));
            } else {
                out.push_str(&format!("- {name}（{}）\n", headings.join("、")));
            }
        }
        out.push('\n');
    }

    // 3. 工作区视图。
    let views = list_md_names(&paths.workspace_okf_dir(ws).join("views"));
    if !views.is_empty() {
        out.push_str("# 视图\n");
        for n in &views {
            out.push_str(&format!("- {n}\n"));
        }
        out.push('\n');
    }

    // 4. 工作区数据源知识。
    let sources = list_md_names(&paths.workspace_okf_dir(ws).join("sources"));
    if !sources.is_empty() {
        out.push_str("# 数据源知识\n");
        for n in &sources {
            out.push_str(&format!("- {n}\n"));
        }
        out.push('\n');
    }

    // 5. 排障记录（pipelines/specific）。
    let recipes = list_md_names(&paths.workspace_okf_dir(ws).join("pipelines").join("specific"));
    if !recipes.is_empty() {
        out.push_str("# 排障记录\n");
        for n in &recipes {
            out.push_str(&format!("- {n}\n"));
        }
        out.push('\n');
    }

    out.trim().to_string()
}

/// 全文搜索全局 + 工作区 OKF（子串匹配，命中取前 6 行预览）。
pub fn search(paths: &OkfPaths, ws: &str, query: &str) -> Vec<SearchHit> {
    let query_lower = query.to_lowercase();
    let mut hits = Vec::new();
    search_dir(&paths.global_okf_dir(), &query_lower, Scope::Global, &mut hits);
    search_dir(&paths.workspace_okf_dir(ws), &query_lower, Scope::Workspace, &mut hits);
    hits
}

// ---------- 扫描辅助 ----------

/// 列出目录下 .md 文件名（不含后缀），按名排序。
fn list_md_names(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                if !stem.is_empty() {
                    names.push(stem.to_string());
                }
            }
        }
    }
    names.sort();
    names
}

/// 列出目录下 .md 文件名 + 指定级别标题索引，按名排序。
fn list_with_headings(dir: &Path, level: usize) -> Vec<(String, Vec<String>)> {
    let mut items = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
            if stem.is_empty() {
                continue;
            }
            let headings = fs::read_to_string(&p)
                .ok()
                .map(|c| markdown::extract_headings(&c, level))
                .unwrap_or_default();
            items.push((stem.to_string(), headings));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

fn search_dir(root: &Path, query_lower: &str, scope: Scope, hits: &mut Vec<SearchHit>) {
    if !root.exists() {
        return;
    }
    for entry in walkdir::WalkDir::new(root).into_iter().flatten() {
        if !entry.path().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();
        let Ok(content) = fs::read_to_string(entry.path()) else { continue };
        // 找到首个匹配行，取其上下文作预览（比固定前 6 行更有用）。
        let matched_line = content
            .lines()
            .enumerate()
            .find(|(_, l)| l.to_lowercase().contains(query_lower))
            .map(|(i, _)| i);
        let Some(ml) = matched_line else { continue };
        let lines: Vec<&str> = content.lines().collect();
        let from = ml.saturating_sub(1);
        let to = (ml + 4).min(lines.len());
        let preview = lines[from..to].join("\n");
        hits.push(SearchHit {
            rel_path: format!("[{}] {}", scope.label(), rel),
            preview,
        });
    }
}

// 静默未使用导入警告（Category 在此文件暂未直接用，保留以备 catalog 结构化扩展）。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okf::model::Category;
    use crate::okf::{Clock, NoopVersioner};
    use std::sync::Arc;

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_ts(&self) -> String {
            "2026-08-14T00:00:00Z".to_string()
        }
    }

    #[test]
    fn summary_lists_global_concept_and_recipe() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        // 全局概念
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Concept, "company", "业务描述", "我们是做零售的", None).unwrap();
        // 排障配方（工作区）
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Recipe, "date_parse", "解决方案", "用 to_date 解析", None).unwrap();

        let s = summary(&paths, "ws", &[]);
        assert!(s.contains("业务概念（全局）"));
        assert!(s.contains("company"));
        assert!(s.contains("排障记录"));
        assert!(s.contains("date_parse"));
    }

    #[test]
    fn summary_renders_table_with_status_and_semantics() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        // 建表骨架（含字段表）
        let cols = vec![("order_id".to_string(), "BIGINT".to_string(), false)];
        store::ensure_table_skeleton(&paths, ver.as_ref(), &FixedClock, "ws", "v_orders", &cols, Some(10)).unwrap();
        // 补字段释义
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Table, "v_orders", "字段 Schema", "| 字段名 | 物理类型 | 业务释义 | 数据约束 |\n|---|---|---|---|\n| `order_id` | BIGINT | 订单编号 | NOT NULL |", None).unwrap();

        let entry = TableRegistryEntry {
            id: "1".into(),
            workspace_path: "ws".into(),
            connection_name: "myshop".into(),
            local_name: "v_orders".into(),
            remote_schema: "public".into(),
            remote_table: "orders".into(),
            db_type: "postgres".into(),
            db_product: "pg".into(),
            db_mode: "standard".into(),
            table_type: "native".into(),
            access_mode: "catalog".into(),
            status: "available".into(),
            unavailable_reason: None,
            last_explored: None,
        };
        let s = summary(&paths, "ws", &[entry]);
        assert!(s.contains("✅ `v_orders` (myshop)"));
        assert!(s.contains("订单编号"));
    }

    #[test]
    fn search_finds_keyword_across_scopes() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Concept, "co", "业务描述", "特别关键词 ALPHA", None).unwrap();
        let hits = search(&paths, "ws", "alpha");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].rel_path.contains("全局"));
        assert!(hits[0].preview.contains("ALPHA"));
    }

    #[test]
    fn search_no_hit_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let hits = search(&paths, "ws", "不存在的东西");
        assert!(hits.is_empty());
    }
}
