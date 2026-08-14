//! Markdown body 纯函数：标题块提取、标题索引、去重。无 I/O。

/// 提取指定标题下的内容块（到下一个同级或更高级标题前），大小写不敏感。
/// 未找到返回 None。`heading` 由调用方保证非空非 all。
pub fn extract_block(content: &str, heading: &str) -> Option<String> {
    let mut block_content = Vec::new();
    let mut recording = false;
    for line in content.lines() {
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

/// 提取指定级别（如 2 → `## `）的标题文本，跳过 frontmatter。
pub fn extract_headings(content: &str, level: usize) -> Vec<String> {
    let prefix = "#".repeat(level) + " ";
    let mut headings = Vec::new();
    let mut started = false; // 已越过任何前导 frontmatter
    let mut in_fm = false;
    for line in content.lines() {
        let t = line.trim();
        if !started {
            if t == "---" {
                in_fm = true;
                started = true;
                continue;
            } else if t.is_empty() {
                continue;
            } else {
                started = true; // 无 frontmatter，内容直接开始
            }
        }
        if in_fm {
            if t == "---" {
                in_fm = false;
            }
            continue;
        }
        if let Some(h) = t.strip_prefix(&prefix) {
            if !h.starts_with('#') {
                headings.push(h.trim().to_string());
            }
        }
    }
    headings
}

/// 在 body 里替换或追加标题板块。heading 匹配（大小写不敏感）则替换其内容
/// （到下一个 `#` 标题前）；不存在则末尾追加 level 级新标题 + content。
pub fn upsert_block(body: &str, heading: &str, content: &str, level: usize) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut skipping = false;
    let mut written = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let ht = trimmed.trim_start_matches('#').trim();
            if skipping {
                skipping = false;
            }
            if ht.eq_ignore_ascii_case(heading) {
                out.push(line.to_string());
                out.push(content.to_string());
                skipping = true;
                written = true;
                continue;
            }
        }
        if !skipping {
            out.push(line.to_string());
        }
    }
    if !written {
        if !out.is_empty() {
            // 与已有内容空行分隔。
            let needs_blank = out.last().map(|l| !l.is_empty()).unwrap_or(false);
            if needs_blank {
                out.push(String::new());
            }
        }
        let prefix = "#".repeat(level);
        out.push(format!("{prefix} {heading}"));
        out.push(content.to_string());
    }
    out.join("\n")
}

/// 去重同标题板块（frontmatter 之外），保留首次出现的板块。
pub fn deduplicate(content: &str) -> String {    let mut clean_lines = Vec::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_block_found() {
        let doc = "# A\nx\n# B\ny";
        assert_eq!(extract_block(doc, "A"), Some("x".to_string()));
        assert_eq!(extract_block(doc, "B"), Some("y".to_string()));
    }

    #[test]
    fn extract_block_case_insensitive() {
        let doc = "## 业务描述\ncontent";
        assert_eq!(extract_block(doc, "业务描述"), Some("content".to_string()));
        assert_eq!(extract_block(doc, "业 务 描述"), None);
    }

    #[test]
    fn extract_block_missing() {
        assert_eq!(extract_block("# A\nx", "Z"), None);
    }

    #[test]
    fn extract_block_stops_at_next_heading() {
        let doc = "# A\nline1\n## sub\nline2\n# B\nline3";
        // 同级或更高级标题前截断：到下一个 `#`（一级）停
        let block = extract_block(doc, "A").unwrap();
        assert!(block.contains("line1"));
        assert!(!block.contains("line3"));
    }

    #[test]
    fn extract_headings_level2_skips_frontmatter() {
        let doc = "---\ntitle: x\n---\n\n## 业务描述\nbody\n## 组织架构\nbody2";
        let hs = extract_headings(doc, 2);
        assert_eq!(hs, vec!["业务描述".to_string(), "组织架构".to_string()]);
    }

    #[test]
    fn extract_headings_no_frontmatter() {
        let hs = extract_headings("## A\n## B", 2);
        assert_eq!(hs, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn extract_headings_level_filter() {
        let doc = "# one\n## two\n### three";
        assert_eq!(extract_headings(doc, 2), vec!["two".to_string()]);
    }

    #[test]
    fn deduplicate_keeps_first_block() {
        let doc = "# A\nfirst\n# A\nsecond";
        let dedup = deduplicate(doc);
        assert!(dedup.contains("first"));
        assert!(!dedup.contains("second"));
    }

    #[test]
    fn deduplicate_preserves_frontmatter() {
        let doc = "---\ntype: T\n# A\n---\n# A\nkeep"; // frontmatter 内含 # 行
        let dedup = deduplicate(doc);
        // frontmatter 行原样保留（# A 在 yaml 内不算标题）
        assert!(dedup.contains("type: T"));
    }

    #[test]
    fn deduplicate_distinct_headings_kept() {
        let doc = "# A\nx\n# B\ny";
        let dedup = deduplicate(doc);
        assert!(dedup.contains("x"));
        assert!(dedup.contains("y"));
    }

    #[test]
    fn upsert_replaces_existing_block() {
        let body = "# A\nold\n# B\nkeep";
        let out = upsert_block(body, "A", "new", 1);
        assert!(out.contains("new"));
        assert!(!out.contains("old"));
        assert!(out.contains("keep"));
    }

    #[test]
    fn upsert_appends_when_missing() {
        let body = "# A\nx";
        let out = upsert_block(body, "关联关系", "rel content", 1);
        assert!(out.contains("# A"));
        assert!(out.contains("# 关联关系"));
        assert!(out.contains("rel content"));
    }

    #[test]
    fn upsert_appends_level2_for_concepts() {
        let out = upsert_block("", "业务描述", "desc", 2);
        assert!(out.contains("## 业务描述"));
        assert!(out.contains("desc"));
    }

    #[test]
    fn upsert_case_insensitive_match() {
        let body = "## 业务描述\nold";
        let out = upsert_block(body, "业务描述", "new", 2);
        assert!(out.contains("new"));
        assert!(!out.contains("old"));
    }
}
