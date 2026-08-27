//! OKF 目录大纲引擎：自动扫描生成 preamble summary + 全文搜索。
//!
//! 大纲的唯一来源——不依赖 index.md，每次实时扫描文件目录。
//! 表清单来自 table_registry（结构化，由调用方传入），其余类别扫描目录。

use std::fs;
use std::path::Path;

use crate::duckdb::attach::workspace_attach_alias;
use crate::model::TableRegistryEntry;
use crate::okf::markdown;
use crate::okf::model::{Scope, SearchHit, TableStatus};
use crate::okf::paths::OkfPaths;
use crate::okf::similarity;
use crate::okf::store;

/// 生成知识库大纲（注入 preamble + list_okf_knowledge 工具共用）。
/// 两级结构：全局知识 / 工作区知识 → 各类别带目录提示 + 计数 + 描述。
pub fn summary(paths: &OkfPaths, ws: &str, table_entries: &[TableRegistryEntry]) -> String {
    let mut out = String::new();

    // ===== 全局知识 =====
    let concepts = list_with_headings(&paths.global_okf_dir().join("concepts"), 2);
    let users = list_with_desc(&paths.global_okf_dir().join("users"));
    if !concepts.is_empty() || !users.is_empty() {
        out.push_str("# 知识库大纲\n\n## 全局知识（跨工作区共享）\n");
        if !concepts.is_empty() {
            out.push_str(&format!("### 业务概念 (concepts/) · {}\n", concepts.len()));
            for (name, headings) in &concepts {
                if headings.is_empty() {
                    out.push_str(&format!("- {name}\n"));
                } else {
                    out.push_str(&format!("- {name}（{}）\n", headings.join("、")));
                }
            }
        }
        if !users.is_empty() {
            out.push_str(&format!("### 用户背景 (users/) · {}\n", users.len()));
            for (name, desc) in &users {
                push_item(&mut out, name, desc.as_deref());
            }
        }
    }

    // ===== 工作区知识 =====
    let selections = render_selections(paths, ws, SUMMARY_SELECTIONS_BUDGET);
    let views = list_with_desc(&paths.workspace_okf_dir(ws).join("views"));
    let sources = list_with_desc(&paths.workspace_okf_dir(ws).join("sources"));
    let recipes = list_with_desc(
        &paths
            .workspace_okf_dir(ws)
            .join("pipelines")
            .join("specific"),
    );
    if !table_entries.is_empty() || !views.is_empty() || !sources.is_empty() || !recipes.is_empty() || !selections.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        } else {
            out.push_str("# 知识库大纲\n\n");
        }
        out.push_str("## 工作区知识\n");
        if !selections.is_empty() {
            out.push_str(&selections);
        }
        if !table_entries.is_empty() {
            out.push_str(&render_registered_tables(paths, ws, table_entries, SUMMARY_TABLES_BUDGET));
        }
        if !views.is_empty() {
            out.push_str(&format!("### 视图 (views/) · {}\n", views.len()));
            for (name, desc) in &views {
                push_item(&mut out, name, desc.as_deref());
            }
        }
        if !sources.is_empty() {
            out.push_str(&format!("### 数据源知识 (sources/) · {}\n", sources.len()));
            for (name, desc) in &sources {
                push_item(&mut out, name, desc.as_deref());
            }
        }
        if !recipes.is_empty() {
            out.push_str(&format!("### 排障配方 (pipelines/specific/) · {}\n", recipes.len()));
            for (name, desc) in &recipes {
                push_item(&mut out, name, desc.as_deref());
            }
        }
    }

    let dup = duplicate_section(paths, ws);
    if !dup.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&dup);
    }

    out.trim().to_string()
}

/// 注入 preamble 的"已注册表"小节的 token 预算。
///
/// 大纲随注册表数量无上限膨胀（每张表的字段释义都进 preamble，而 preamble
/// 在每个 LLM 请求上全量重发）。预算内的表保留完整信息，之外的降级为名称
/// 清单，超出 6 张时折叠成一行提示（用 list_tables 按需查看）。
/// 当前 31 张注册表的完整大纲约 900 tokens，预算 1000 留有余量且封顶增长。
const SUMMARY_TABLES_BUDGET: u64 = 1000;

/// 单张表的状态行：`- ✅ \`v_xxx\` (别名 db_xxx) [下推表·禁直查] — 不可用原因`。
/// 徽标必须自带行为指令——曾用 [pushdown] 时模型把它读反成「视图已下推、
/// 可直接查」，导致 15 次 24–45s 的全表拉取慢查询（2026-08-27 复盘）。
fn entry_status_line(e: &TableRegistryEntry) -> String {
    let icon = TableStatus::from_str(&e.status).icon();
    let mode = if e.access_mode == "pushdown" { " [下推表·禁直查]" } else { "" };
    let reason = if e.status != "available" {
        e.unavailable_reason
            .as_ref()
            .map(|r| format!(" — 不可用: {r}"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(
        "- {icon} `{}` (别名 {}){}{}\n",
        e.local_name,
        workspace_attach_alias(&e.connection_name),
        mode,
        reason
    )
}

/// 渲染带预算的"已注册表"小节。按 `last_explored` 降序近似"最近使用"：
/// 预算内输出完整条目（状态行 + 字段释义 + 关联关系），预算外仅名称行，
/// 积压超过 6 张时折叠为一行提示。至少保证第一张表完整输出。
fn render_registered_tables(
    paths: &OkfPaths,
    ws: &str,
    table_entries: &[TableRegistryEntry],
    budget: u64,
) -> String {
    let mut out = format!("### 已注册表 · {}\n", table_entries.len());
    let mut entries: Vec<&TableRegistryEntry> = table_entries.iter().collect();
    entries.sort_by(|a, b| {
        b.last_explored
            .unwrap_or(0)
            .cmp(&a.last_explored.unwrap_or(0))
            .then_with(|| a.local_name.cmp(&b.local_name))
    });

    let mut used = crate::usage::estimate_tokens(&out);
    let mut emitted_full = false;
    let mut name_only: Vec<&TableRegistryEntry> = Vec::new();
    for e in entries {
        let mut block = entry_status_line(e);
        let ts = store::table_semantics(paths, ws, &e.local_name);
        block.push_str(&render_columns(&ts.columns));
        block.push_str(&render_relations(&ts.relations));
        let cost = crate::usage::estimate_tokens(&block);
        if emitted_full && used + cost > budget {
            name_only.push(e);
        } else {
            out.push_str(&block);
            used += cost;
            emitted_full = true;
        }
    }
    if name_only.is_empty() {
        return out;
    }
    if name_only.len() <= 6 {
        for e in &name_only {
            out.push_str(&entry_status_line(e));
        }
    } else {
        let preview: Vec<&str> = name_only
            .iter()
            .take(4)
            .map(|e| e.local_name.as_str())
            .collect();
        out.push_str(&format!(
            "- （另有 {} 张表：{}…，需要时用 list_tables / list_okf_knowledge 查看）\n",
            name_only.len(),
            preview.join("、")
        ));
    }
    out
}

/// `- name — description`（有描述）或 `- name`（无）。
fn push_item(out: &mut String, name: &str, desc: Option<&str>) {
    match desc {
        Some(d) if !d.is_empty() => out.push_str(&format!("- {name} — {d}\n")),
        _ => out.push_str(&format!("- {name}\n")),
    }
}

/// 注入 preamble 的"选表经验"小节的 token 预算。
/// 经验条目通常个位数，但随主题积累无上限；超预算降级为名称清单，
/// 与已注册表小节同模式，防止 preamble 随知识积累无上限膨胀。
const SUMMARY_SELECTIONS_BUDGET: u64 = 400;

/// 渲染"选表经验"小节（selections/）：每条一行摘要——
/// `- 销量分析 — 首选 [[v_orders_daily]]；交叉验证 [[v_orders_detail]]`。
/// 表名来自正文 `## 首选表` / `## 交叉验证表` 块的列表行；两块都缺失时降级为
/// `- name — description`。至少保证第一条完整输出；超预算降级为名称行，
/// 积压超过 6 条时折叠为一行提示。无经验文件返回空串。
fn render_selections(paths: &OkfPaths, ws: &str, budget: u64) -> String {
    let dir = paths.workspace_okf_dir(ws).join("selections");
    let items = list_with_desc(&dir);
    if items.is_empty() {
        return String::new();
    }
    let mut out = format!("### 选表经验 (selections/) · {}\n", items.len());
    let mut used = crate::usage::estimate_tokens(&out);
    let mut emitted_full = false;
    let mut name_only: Vec<&(String, Option<String>)> = Vec::new();
    for item in &items {
        let (name, desc) = item;
        let content = fs::read_to_string(dir.join(format!("{name}.md"))).unwrap_or_default();
        let first = block_table_tokens(&content, "首选表");
        let cross = block_table_tokens(&content, "交叉验证表");
        let mut parts: Vec<String> = Vec::new();
        if !first.is_empty() {
            parts.push(format!(
                "首选 {}",
                first.iter().map(|t| format!("[[{t}]]")).collect::<Vec<_>>().join("、")
            ));
        }
        if !cross.is_empty() {
            parts.push(format!(
                "交叉验证 {}",
                cross.iter().map(|t| format!("[[{t}]]")).collect::<Vec<_>>().join("、")
            ));
        }
        let line = if parts.is_empty() {
            match desc.as_deref() {
                Some(d) if !d.is_empty() => format!("- {name} — {d}\n"),
                _ => format!("- {name}\n"),
            }
        } else {
            format!("- {name} — {}\n", parts.join("；"))
        };
        let cost = crate::usage::estimate_tokens(&line);
        if emitted_full && used + cost > budget {
            name_only.push(item);
        } else {
            out.push_str(&line);
            used += cost;
            emitted_full = true;
        }
    }
    match name_only.len() {
        0 => {}
        n if n <= 6 => {
            for (name, desc) in &name_only {
                push_item(&mut out, name, desc.as_deref());
            }
        }
        _ => {
            let preview: Vec<&str> = name_only.iter().take(4).map(|(n, _)| n.as_str()).collect();
            out.push_str(&format!(
                "- （另有 {} 条选表经验：{}…，需要时用 list_okf_knowledge / search_okf_knowledge 查看）\n",
                name_only.len(),
                preview.join("、")
            ));
        }
    }
    out
}

/// 提取指定标题块中列表行的表名 token（[[内链]] 或 `代码` 标记），最多 3 个。
fn block_table_tokens(content: &str, heading: &str) -> Vec<String> {
    let Some(block) = markdown::extract_block(content, heading) else {
        return Vec::new();
    };
    let mut tokens: Vec<String> = Vec::new();
    for line in block.lines() {
        if let Some(tok) = list_table_token(line) {
            if !tokens.contains(&tok) {
                tokens.push(tok);
            }
        }
        if tokens.len() >= 3 {
            break;
        }
    }
    tokens
}

/// 从 `- ...` 列表行提取第一个 `[[内链]]` 或 `` `代码` `` 标记的表名。
fn list_table_token(line: &str) -> Option<String> {
    let t = line.trim().strip_prefix("- ")?;
    for (open, close) in [("[[", "]]"), ("`", "`")] {
        if let Some(rest) = t.trim_start().strip_prefix(open) {
            if let Some(end) = rest.find(close) {
                let tok = rest[..end].trim();
                if !tok.is_empty() {
                    return Some(tok.to_string());
                }
            }
        }
    }
    None
}

/// 渲染字段释义行（`  字段释义: \`col\`: comment; ...`），空则空串。
fn render_columns(cols: &[crate::okf::model::ColumnSemantic]) -> String {
    let parts: Vec<String> = cols.iter()
        .filter(|c| !c.comment.is_empty())
        .map(|c| format!("`{}`: {}", c.name, c.comment))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("  字段释义: {}\n", parts.join("; "))
    }
}

/// 渲染关联关系行（`  关联: - \`col\` → [[target]].\`col\` (N:1) ...`），空则空串。
fn render_relations(rels: &[crate::okf::model::Relation]) -> String {
    if rels.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = rels.iter().map(|r| {
        if r.local_col.is_empty() {
            format!("- [[{}]] ({}) {}", r.target_table, r.cardinality.to_str(), r.description.as_deref().unwrap_or(""))
        } else {
            format!("- `{}` {} [[{}]].`{}` ({})", r.local_col, r.direction.to_arrow(), r.target_table, r.target_col, r.cardinality.to_str())
        }
    }).collect();
    format!("  关联: {}\n", parts.join(" "))
}

/// 全文搜索全局 + 工作区 OKF。query 按空白分词：任一 token 命中即视为命中，
/// 按命中 token 数降序排列——多关键词查询不再因"整句子串不匹配"而恒为 0 条。
pub fn search(paths: &OkfPaths, ws: &str, query: &str) -> Vec<SearchHit> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, SearchHit)> = Vec::new();
    search_dir(&paths.global_okf_dir(), &tokens, Scope::Global, &mut scored);
    search_dir(&paths.workspace_okf_dir(ws), &tokens, Scope::Workspace, &mut scored);
    // 稳定排序：命中 token 多的文件排前面（降序），同分保持扫描顺序。
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, h)| h).collect()
}

// ---------- 扫描辅助 ----------

/// 列出目录下 .md 文件名 + frontmatter description，按名排序。
fn list_with_desc(dir: &Path) -> Vec<(String, Option<String>)> {
    let mut items = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.is_empty() {
                continue;
            }
            let desc = fs::read_to_string(&p)
                .ok()
                .and_then(|c| crate::okf::frontmatter::split_document(&c).0)
                .and_then(|fm| fm.get("description").filter(|s| !s.is_empty()).map(|s| s.to_string()));
            items.push((stem.to_string(), desc));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

/// 列出目录下 .md 文件名 + 完整 frontmatter + 标题索引，按名排序。
fn list_with_metadata(dir: &Path, heading_level: Option<usize>) -> Vec<(String, crate::okf::frontmatter::Frontmatter, Vec<String>)> {
    let mut items = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.is_empty() {
                continue;
            }
            let content = match fs::read_to_string(&p) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let (fm, _) = crate::okf::frontmatter::split_document(&content);
            let fm = fm.unwrap_or_default();
            let headings = heading_level
                .map(|lvl| markdown::extract_headings(&content, lvl))
                .unwrap_or_default();
            items.push((stem.to_string(), fm, headings));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

/// 取 frontmatter 字段的日期部分（ISO → YYYY-MM-DD），无则空。
fn date_of(fm: &crate::okf::frontmatter::Frontmatter, key: &str) -> String {
    fm.get(key).map(|s| s.get(..10).unwrap_or("").to_string()).unwrap_or_default()
}

/// 生成带完整元数据的大纲（list_okf_knowledge 工具用，比 summary 丰富）。
/// 每条展示 type/description/created_at/updated_at +（concepts）标题索引。
pub fn outline(paths: &OkfPaths, ws: &str, table_entries: &[TableRegistryEntry]) -> String {
    let mut out = String::new();
    let mut started = false;

    // ===== 全局知识 =====
    let concepts = list_with_metadata(&paths.global_okf_dir().join("concepts"), Some(2));
    let users = list_with_metadata(&paths.global_okf_dir().join("users"), None);
    if !concepts.is_empty() || !users.is_empty() {
        out.push_str("# 知识库大纲\n\n## 全局知识（跨工作区共享）\n");
        started = true;
        if !concepts.is_empty() {
            push_metadata_section(&mut out, "业务概念 (concepts/)", &concepts, true);
        }
        if !users.is_empty() {
            push_metadata_section(&mut out, "用户背景 (users/)", &users, false);
        }
    }

    // ===== 工作区知识 =====
    let selections = list_with_metadata(&paths.workspace_okf_dir(ws).join("selections"), None);
    let views = list_with_metadata(&paths.workspace_okf_dir(ws).join("views"), None);
    let sources = list_with_metadata(&paths.workspace_okf_dir(ws).join("sources"), None);
    let recipes = list_with_metadata(
        &paths.workspace_okf_dir(ws).join("pipelines").join("specific"),
        None,
    );
    if !table_entries.is_empty() || !views.is_empty() || !sources.is_empty() || !recipes.is_empty() || !selections.is_empty() {
        if started {
            out.push('\n');
        } else {
            out.push_str("# 知识库大纲\n\n");
        }
        out.push_str("## 工作区知识\n");
        if !selections.is_empty() {
            push_metadata_section(&mut out, "选表经验 (selections/)", &selections, false);
        }
        if !table_entries.is_empty() {
            out.push_str(&format!("### 已注册表 · {}\n", table_entries.len()));
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
                let ts = store::table_semantics(paths, ws, &e.local_name);
                out.push_str(&format!(
                    "- {icon} `{}` (别名 {}){}{}\n",
                    e.local_name, workspace_attach_alias(&e.connection_name), mode, reason
                ));
                out.push_str(&render_columns(&ts.columns));
                out.push_str(&render_relations(&ts.relations));
            }
        }
        if !views.is_empty() {
            push_metadata_section(&mut out, "视图 (views/)", &views, false);
        }
        if !sources.is_empty() {
            push_metadata_section(&mut out, "数据源知识 (sources/)", &sources, false);
        }
        if !recipes.is_empty() {
            push_metadata_section(&mut out, "排障配方 (pipelines/specific/)", &recipes, false);
        }
    }

    let dup = duplicate_section(paths, ws);
    if !dup.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&dup);
    }

    out.trim().to_string()
}

/// 疑似重复知识小节：全局 concepts/users + 工作区 views/sources/recipes 分组检测。
/// 无重复返回空串。组内按名排序，组间按大小降序，最多展示 6 组（控制 preamble 体积）。
fn duplicate_section(paths: &OkfPaths, ws: &str) -> String {
    let global = paths.global_okf_dir();
    let wksp = paths.workspace_okf_dir(ws);
    let mut sections: Vec<(&str, Vec<Vec<String>>)> = Vec::new();
    for (label, dir) in [
        ("concepts", global.join("concepts")),
        ("users", global.join("users")),
        ("selections", wksp.join("selections")),
        ("views", wksp.join("views")),
        ("sources", wksp.join("sources")),
        ("recipes", wksp.join("pipelines").join("specific")),
    ] {
        let groups = similarity::duplicate_groups(&dir);
        if !groups.is_empty() {
            sections.push((label, groups));
        }
    }
    if sections.is_empty() {
        return String::new();
    }
    let mut flat: Vec<(&str, Vec<String>)> = sections
        .into_iter()
        .flat_map(|(label, groups)| groups.into_iter().map(move |g| (label, g)))
        .collect();
    flat.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    flat.truncate(6);

    let mut out = String::from("## ⚠️ 疑似重复知识（同一主题应合并为一个文件）\n");
    for (label, g) in flat {
        out.push_str(&format!("- {label}: {}\n", g.join(" ≈ ")));
    }
    out
}

/// 渲染一个元数据小节：每条 - name / type / desc / created / updated [+ headings]。
fn push_metadata_section(
    out: &mut String,
    title: &str,
    items: &[(String, crate::okf::frontmatter::Frontmatter, Vec<String>)],
    show_headings: bool,
) {
    out.push_str(&format!("### {title} · {}\n", items.len()));
    for (name, fm, headings) in items {
        let ty = fm.get("type").unwrap_or("");
        let desc = fm.get("description").unwrap_or("");
        let created = date_of(fm, "created_at");
        let updated = date_of(fm, "updated_at");
        out.push_str(&format!("- **{name}**\n"));
        out.push_str(&format!("  类型: {ty}"));
        if !desc.is_empty() {
            out.push_str(&format!(" | 描述: {desc}"));
        }
        if !created.is_empty() || !updated.is_empty() {
            out.push_str(&format!(" | 创建: {created} 更新: {updated}"));
        }
        out.push('\n');
        if show_headings && !headings.is_empty() {
            out.push_str(&format!("  板块: {}\n", headings.join("、")));
        }
    }
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

fn search_dir(root: &Path, tokens: &[String], scope: Scope, out: &mut Vec<(usize, SearchHit)>) {
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
        // 文件级评分 = 命中的 token 数；预览取命中 token 最多的那一行。
        let mut best_line: Option<(usize, usize)> = None;
        for (i, l) in content.lines().enumerate() {
            let line_lower = l.to_lowercase();
            let score = tokens.iter().filter(|t| line_lower.contains(t.as_str())).count();
            if score == 0 {
                continue;
            }
            let better = match best_line {
                Some((c, _)) => score > c,
                None => true,
            };
            if better {
                best_line = Some((score, i));
            }
        }
        let Some((score, matched_line)) = best_line else { continue };
        let lines: Vec<&str> = content.lines().collect();
        let from = matched_line.saturating_sub(1);
        let to = (matched_line + 4).min(lines.len());
        out.push((
            score,
            SearchHit {
                rel_path: format!("[{}] {}", scope.label(), rel),
                preview: lines[from..to].join("\n"),
            },
        ));
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
        assert!(s.contains("# 知识库大纲"));
        assert!(s.contains("## 全局知识"));
        assert!(s.contains("### 业务概念 (concepts/) · 1"));
        assert!(s.contains("company"));
        assert!(s.contains("## 工作区知识"));
        assert!(s.contains("### 排障配方 (pipelines/specific/) · 1"));
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
            kind: "table".into(),
        };
        let s = summary(&paths, "ws", &[entry]);
        assert!(s.contains("### 已注册表 · 1"));
        assert!(s.contains("✅ `v_orders` (别名 db_myshop)"));
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

    #[test]
    fn outline_shows_full_metadata_per_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Concept, "co", "业务描述", "body", Some("一句话描述")).unwrap();
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Recipe, "fix", "解决方案", "用 to_date", Some("日期解析排障")).unwrap();

        let s = outline(&paths, "ws", &[]);
        // 概念条目带完整元数据
        assert!(s.contains("**co**"));
        assert!(s.contains("类型: Business Concept"));
        assert!(s.contains("描述: 一句话描述"));
        assert!(s.contains("创建: 2026-08-14"));
        assert!(s.contains("更新: 2026-08-14"));
        assert!(s.contains("板块: 业务描述"));
        // 排障配方也带元数据
        assert!(s.contains("**fix**"));
        assert!(s.contains("类型: Recipe"));
        assert!(s.contains("描述: 日期解析排障"));
    }

    #[test]
    fn summary_and_outline_flag_duplicate_concepts() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Concept, "org_consult_center", "业务描述", "a", Some("咨询中心组织架构权威版本")).unwrap();
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Concept, "consult_center_org", "业务描述", "b", Some("咨询中心组织架构旧快照")).unwrap();

        for s in [summary(&paths, "ws", &[]), outline(&paths, "ws", &[])] {
            assert!(s.contains("疑似重复知识"), "missing dup section in: {s}");
            assert!(s.contains("concepts: consult_center_org ≈ org_consult_center"));
        }
    }

#[test]
    fn no_duplicates_no_section() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Concept, "company_profile", "业务描述", "公司背景", Some("公司主营业务背景")).unwrap();
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Recipe, "date_parse", "解决方案", "用 to_date 解析", Some("日期解析排障配方")).unwrap();

        let s = summary(&paths, "ws", &[]);
        assert!(!s.contains("疑似重复知识"));
    }

    #[test]
    fn registered_tables_respect_token_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let mk = |n: usize, last: i64| TableRegistryEntry {
            id: format!("t{n}"),
            workspace_path: "ws".into(),
            connection_name: "db".into(),
            local_name: format!("v_t{n}"),
            remote_schema: "public".into(),
            remote_table: format!("t{n}"),
            db_type: "postgres".into(),
            db_product: "pg".into(),
            db_mode: "standard".into(),
            table_type: "native".into(),
            access_mode: "catalog".into(),
            status: "available".into(),
            unavailable_reason: None,
            last_explored: Some(last),
            kind: "table".into(),
        };
        // 1 张热表（last_explored 最大）+ 20 张冷表。
        let mut entries: Vec<TableRegistryEntry> = vec![mk(0, 100)];
        for n in 1..21 {
            entries.push(mk(n, 1));
        }
        let out = render_registered_tables(&paths, "ws", &entries, 20);
        // 热表完整输出；预算耗尽后其余 20 张折叠为一行，预览前 4 个冷名。
        assert!(out.contains("`v_t0` (别名 db_db)"));          // 热表状态行在
        assert!(out.contains("另有 20 张表"));                   // 折叠提示在
        assert!(out.contains("v_t1、v_t10、v_t11、v_t12"));    // 预览列冷表名（按名排序）
        assert!(!out.contains("v_t13"));                       // 超出预览的不再逐个列出
        // 折叠行体积有上限：总输出远小于 21 × 完整条目。
        assert!(crate::usage::estimate_tokens(&out) < 150);
    }

    #[test]
    fn summary_renders_selections_with_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Selection, "sales_analysis", "首选表", "- [[v_orders_daily]] @demo — 订单日汇总，口径稳定\n- [[v_orders_sum]] @demo — 品牌口径汇总", None).unwrap();
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Selection, "sales_analysis", "交叉验证表", "- [[v_orders_detail]] @demo — 明细互证", None).unwrap();

        let s = summary(&paths, "ws", &[]);
        assert!(s.contains("### 选表经验 (selections/) · 1"));
        assert!(s.contains("- sales_analysis — 首选 [[v_orders_daily]]、[[v_orders_sum]]；交叉验证 [[v_orders_detail]]"));
    }

    #[test]
    fn selections_without_blocks_fall_back_to_desc() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Selection, "draft", "适用问题", "还没填表的草稿", Some("占位经验")).unwrap();

        let s = summary(&paths, "ws", &[]);
        assert!(s.contains("### 选表经验 (selections/) · 1"));
        assert!(s.contains("- draft — 占位经验"));
        assert!(!s.contains("首选 ["));
    }

    #[test]
    fn selections_respect_token_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = OkfPaths::new(tmp.path().to_path_buf());
        let ver: Arc<dyn crate::okf::Versioner> = Arc::new(NoopVersioner);
        for n in 0..10 {
            let name = format!("topic_{n:02}");
            store::write(&paths, ver.as_ref(), &FixedClock, "ws", Category::Selection, &name, "首选表", &format!("- [[v_some_rather_long_table_name_{n}]] @demo — 说明文字较长会消耗预算"), None).unwrap();
        }
        let out = render_selections(&paths, "ws", 20);
        // 第一条完整输出；其余 9 条超出折叠阈值 6，收敛为一行提示。
        assert!(out.contains("首选 [[v_some_rather_long_table_name_0]]"));
        assert!(out.contains("另有 9 条选表经验"));
        assert!(crate::usage::estimate_tokens(&out) < 150);
    }
}
