//! YAML frontmatter 解析/序列化（简单 key:value，无外部依赖）。
//!
//! 只识别 `key: value` 行；未知字段保留（便于人手编辑不丢失）。
//! 缺失字段给空（容错，满足"用户直编"场景）。

/// 有序 frontmatter 条目。保留未知字段，已知字段见 model/timestamp。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frontmatter {
    pub entries: Vec<(String, String)>,
}

impl Frontmatter {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// 设置字段：已存在则更新首个匹配，否则追加。
    pub fn set(&mut self, key: &str, val: &str) {
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| k == key) {
            slot.1 = val.to_string();
        } else {
            self.entries.push((key.to_string(), val.to_string()));
        }
    }

    /// 从 frontmatter 文本（不含首尾 `---` 行）解析。
    pub fn parse(text: &str) -> Self {
        let mut fm = Self::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(idx) = trimmed.find(':') {
                let k = trimmed[..idx].trim().to_string();
                let v = trimmed[idx + 1..].trim().to_string();
                if !k.is_empty() {
                    fm.set(&k, &v);
                }
            }
        }
        fm
    }

    /// 序列化为 frontmatter 块（含首尾 `---`）。空则返回空串。
    pub fn serialize(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let mut s = String::from("---\n");
        for (k, v) in &self.entries {
            s.push_str(k);
            s.push_str(": ");
            s.push_str(v);
            s.push('\n');
        }
        s.push_str("---\n");
        s
    }
}

/// 把文件内容拆为 `(frontmatter, body)`。无 frontmatter 或未闭合 → `(None, 全文)`。
pub fn split_document(content: &str) -> (Option<Frontmatter>, String) {
    let lines: Vec<&str> = content.lines().collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return (None, content.to_string());
    }
    // 在首行之后找闭合 `---`。
    let close = lines.iter().skip(1).position(|l| l.trim() == "---");
    let Some(close_idx) = close else {
        return (None, content.to_string()); // 未闭合，视为无 frontmatter
    };
    let fm_text = lines[1..1 + close_idx].join("\n");
    // 去掉 frontmatter 与 body 之间的分隔空行，给消费者干净 body。
    let body = lines[2 + close_idx..]
        .join("\n")
        .trim_start_matches('\n')
        .to_string();
    (Some(Frontmatter::parse(&fm_text)), body)
}

/// 把 frontmatter + body 拼回文件内容。
pub fn join_document(fm: Option<&Frontmatter>, body: &str) -> String {
    match fm {
        Some(f) if !f.entries.is_empty() => {
            let mut s = f.serialize();
            s.push('\n');
            s.push_str(body);
            s
        }
        _ => body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_fields() {
        let fm = Frontmatter::parse("type: Business Concept\ntitle: foo\ndescription: hi");
        assert_eq!(fm.get("type"), Some("Business Concept"));
        assert_eq!(fm.get("title"), Some("foo"));
        assert_eq!(fm.get("description"), Some("hi"));
        assert!(fm.get("missing").is_none());
    }

    #[test]
    fn set_updates_and_inserts() {
        let mut fm = Frontmatter::new();
        fm.set("title", "a");
        fm.set("title", "b"); // update
        assert_eq!(fm.get("title"), Some("b"));
        assert_eq!(fm.entries.len(), 1);
        fm.set("type", "X"); // insert
        assert_eq!(fm.entries.len(), 2);
    }

    #[test]
    fn parse_ignores_blank_and_comments() {
        let fm = Frontmatter::parse("\n# a comment\ntitle: keep\n");
        assert_eq!(fm.entries.len(), 1);
        assert_eq!(fm.get("title"), Some("keep"));
    }

    #[test]
    fn serialize_roundtrip() {
        let mut fm = Frontmatter::new();
        fm.set("type", "T");
        fm.set("title", "n");
        let s = fm.serialize();
        assert!(s.starts_with("---\n"));
        assert!(s.ends_with("---\n"));
        assert!(s.contains("type: T"));
        // round-trip
        let body = &s["---\n".len()..s.len() - "---\n".len()];
        let parsed = Frontmatter::parse(body);
        assert_eq!(parsed.get("type"), Some("T"));
        assert_eq!(parsed.get("title"), Some("n"));
    }

    #[test]
    fn serialize_empty_is_empty() {
        assert!(Frontmatter::new().serialize().is_empty());
    }

    #[test]
    fn split_with_frontmatter() {
        let doc = "---\ntype: T\ntitle: x\n---\n\n# Heading\nbody";
        let (fm, body) = split_document(doc);
        let fm = fm.unwrap();
        assert_eq!(fm.get("type"), Some("T"));
        assert!(body.contains("# Heading"));
        assert!(body.contains("body"));
        assert!(!body.contains("type:"));
    }

    #[test]
    fn split_without_frontmatter() {
        let doc = "# Just body\nline";
        let (fm, body) = split_document(doc);
        assert!(fm.is_none());
        assert_eq!(body, doc);
    }

    #[test]
    fn split_unclosed_frontmatter() {
        let doc = "---\ntype: T\nbody without close";
        let (fm, _body) = split_document(doc);
        assert!(fm.is_none()); // 未闭合视为无 fm
    }

    #[test]
    fn join_roundtrip_preserves_body() {
        let mut fm = Frontmatter::new();
        fm.set("type", "T");
        let body = "# H\ncontent";
        let doc = join_document(Some(&fm), body);
        let (fm2, body2) = split_document(&doc);
        assert_eq!(fm2.unwrap().get("type"), Some("T"));
        assert_eq!(body2, body);
    }

    #[test]
    fn preserves_unknown_fields() {
        // 人手编辑可能加自定义字段，解析→序列化应保留。
        let fm = Frontmatter::parse("type: T\ncustom_field: hello");
        let s = fm.serialize();
        assert!(s.contains("custom_field: hello"));
    }
}
