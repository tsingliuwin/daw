//! 知识相似度检测：写入防重（新条目 vs 同类目既有条目）+ 疑似重复分组。
//!
//! 信号只取文件名 + frontmatter description，确定性计算（无嵌入/外部依赖）：
//! - 特征：ASCII 词（≥2 字符）+ CJK 连续段字符 bigram；
//! - 打分：特征集合 Dice 系数，name 与 description 分开算。
//!
//! 阈值偏向召回：漏报 = 又多一个重复文件；误报代价低——agent 多一步
//! `confirm_new` 确认即可。阈值用真实 concepts 语料校准（见测试）。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// name 相似阈值（短而精确，要求高一点）。
const NAME_THRESHOLD: f64 = 0.5;
/// description 相似阈值（长文本特征被稀释，略低）。
const DESC_THRESHOLD: f64 = 0.38;
/// 分组用（name+desc 合并特征）阈值。
const GROUP_THRESHOLD: f64 = 0.4;
/// 无区分度的占位描述（旧版自动 seed 遗留标记），视同无描述。
const PLACEHOLDER_DESCS: &[&str] = &["自动初始化的 okf 文档"];
/// 同一描述在目录内出现 ≥3 次视为模板/占位（无区分度），视同无描述。
const BOILERPLATE_FREQ: usize = 3;

/// 相似候选条目。
#[derive(Debug, Clone)]
pub struct SimilarCandidate {
    pub name: String,
    pub description: String,
    pub score: f64,
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

fn push_word(word: &mut String, set: &mut HashSet<String>) {
    if word.chars().count() >= 2 {
        set.insert(word.clone());
    }
    word.clear();
}

fn push_cjk(cjk: &mut String, set: &mut HashSet<String>) {
    let chars: Vec<char> = cjk.chars().collect();
    for w in chars.windows(2) {
        set.insert(format!("{}{}", w[0], w[1]));
    }
    cjk.clear();
}

/// 提取特征集合：ASCII 词 + CJK 段 bigram（单个 CJK 字太泛，丢弃）。
fn features(s: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut word = String::new();
    let mut cjk = String::new();
    for ch in s.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            push_cjk(&mut cjk, &mut set);
            word.push(ch);
        } else if is_cjk(ch) {
            push_word(&mut word, &mut set);
            cjk.push(ch);
        } else {
            push_word(&mut word, &mut set);
            push_cjk(&mut cjk, &mut set);
        }
    }
    push_word(&mut word, &mut set);
    push_cjk(&mut cjk, &mut set);
    set
}

/// Dice 系数：`2|A∩B| / (|A|+|B|)`。空集合无意义，返回 0。
fn dice(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    (2.0 * inter as f64) / (a.len() + b.len()) as f64
}

/// (name_dice, desc_dice)。
fn pair_scores(name_a: &str, desc_a: &str, name_b: &str, desc_b: &str) -> (f64, f64) {
    (
        dice(&features(name_a), &features(name_b)),
        dice(&features(desc_a), &features(desc_b)),
    )
}

/// 综合相似度（分组用）：name 与 desc 的 Dice 取最大值。
pub fn similarity_score(name_a: &str, desc_a: &str, name_b: &str, desc_b: &str) -> f64 {
    let (n, d) = pair_scores(name_a, desc_a, name_b, desc_b);
    n.max(d)
}

/// 列出目录下 .md 文件 (文件名, frontmatter description)，按名排序。
/// 占位/模板描述（旧版 seed 标记，或目录内 ≥3 个文件共用同一描述）视同无描述。
fn entries_with_desc(dir: &Path) -> Vec<(String, String)> {
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
            let desc = fs::read_to_string(&p)
                .ok()
                .and_then(|c| crate::okf::frontmatter::split_document(&c).0)
                .and_then(|fm| fm.get("description").map(|s| s.to_string()))
                .unwrap_or_default();
            items.push((stem.to_string(), desc));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));

    // 统计描述频次，≥BOILERPLATE_FREQ 的共用描述视为模板，失去区分度。
    let mut freq: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (_, d) in &items {
        if !d.is_empty() {
            *freq.entry(d.as_str()).or_default() += 1;
        }
    }
    let boilerplate_set: HashSet<String> = freq
        .into_iter()
        .filter(|(_, c)| *c >= BOILERPLATE_FREQ)
        .map(|(d, _)| d.to_string())
        .collect();
    items
        .into_iter()
        .map(|(n, d)| {
            let boilerplate = PLACEHOLDER_DESCS.contains(&d.trim().to_lowercase().as_str())
                || boilerplate_set.contains(&d);
            (n, if boilerplate { String::new() } else { d })
        })
        .collect()
}

/// 在目录中找与 (name, description) 疑似相似的既有条目，按分数降序。
/// 同名条目跳过（那是覆盖写入，不是重复）。
pub fn find_similar_in_dir(
    dir: &Path,
    name: &str,
    description: Option<&str>,
) -> Vec<SimilarCandidate> {
    let desc = description.unwrap_or("");
    let mut out = Vec::new();
    for (existing, existing_desc) in entries_with_desc(dir) {
        if existing.eq_ignore_ascii_case(name.trim()) {
            continue;
        }
        let (n, d) = pair_scores(name, desc, &existing, &existing_desc);
        if n >= NAME_THRESHOLD || d >= DESC_THRESHOLD {
            out.push(SimilarCandidate {
                name: existing,
                description: existing_desc,
                score: n.max(d),
            });
        }
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// 目录内疑似重复分组：两两相似 ≥ GROUP_THRESHOLD 的传递闭包（并查集）。
/// 返回组（组内按名排序），只含 ≥2 个成员的组，按大小降序。
pub fn duplicate_groups(dir: &Path) -> Vec<Vec<String>> {
    let items = entries_with_desc(dir);
    let n = items.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut Vec<usize>, mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }

    for i in 0..n {
        for j in (i + 1)..n {
            let (na, da) = &items[i];
            let (nb, db) = &items[j];
            if similarity_score(na, da, nb, db) >= GROUP_THRESHOLD {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    let mut buckets: std::collections::HashMap<usize, Vec<String>> = std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        buckets.entry(root).or_default().push(items[i].0.clone());
    }
    let mut groups: Vec<Vec<String>> = buckets
        .into_values()
        .filter(|g| g.len() >= 2)
        .collect();
    for g in &mut groups {
        g.sort();
    }
    groups.sort_by(|a, b| b.len().cmp(&a.len()));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_latin_words() {
        let f = features("consult_center_org");
        assert!(f.contains("consult"));
        assert!(f.contains("center"));
        assert!(f.contains("org"));
    }

    #[test]
    fn features_cjk_bigrams() {
        let f = features("咨询中心组织架构");
        assert!(f.contains("咨询"));
        assert!(f.contains("组织"));
        assert!(f.contains("架构"));
        // 单个 CJK 字不成 bigram，不产生特征
        assert!(features("表").is_empty());
    }

    #[test]
    fn reordered_tokens_are_similar() {
        // 真实案例：consult_center_org vs org_consult_center
        let s = similarity_score("consult_center_org", "", "org_consult_center", "");
        assert!(s >= 0.9, "score={s}");
    }

    #[test]
    fn near_identical_names_are_similar() {
        // 真实案例：只差一个字母
        let s = similarity_score("consult_center_org", "", "consult_center_organ", "");
        assert!(s >= 0.5, "score={s}");
    }

    #[test]
    fn shared_prefix_names_are_similar() {
        // 真实案例：lead_new_old_classification vs lead_new_old_definition
        let s = similarity_score("lead_new_old_classification", "", "lead_new_old_definition", "");
        assert!(s >= 0.5, "score={s}");
    }

    #[test]
    fn unrelated_names_are_not_similar() {
        let s = similarity_score("company_profile", "", "consult_center_org", "");
        assert!(s < 0.3, "score={s}");
    }

    #[test]
    fn descriptions_bridge_naming_gap() {
        // 中英文命名不同、靠 description 桥接（真实案例简化）
        let a = ("new_org_doc", "咨询中心组织架构权威版本，含管辖线与负责人口径");
        let b = ("another_file", "咨询中心组织架构 SCRM 主数据与供给表口径，含命名映射");
        let s = similarity_score(a.0, a.1, b.0, b.1);
        assert!(s >= GROUP_THRESHOLD, "score={s}");
    }

    #[test]
    fn unrelated_descriptions_are_not_similar() {
        let s = similarity_score(
            "a", "公司背景介绍，主营留学咨询业务",
            "b", "日期解析失败的排障配方，用 to_date 处理",
        );
        assert!(s < GROUP_THRESHOLD, "score={s}");
    }

    #[test]
    fn find_similar_returns_sorted_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(
            dir.join("org_consult_center.md"),
            "---\ndescription: 咨询中心组织架构权威版本\n---\nbody",
        ).unwrap();
        fs::write(dir.join("company_profile.md"), "---\ndescription: 公司背景\n---\nbody").unwrap();

        let hits = find_similar_in_dir(dir, "consult_center_org", Some("咨询中心组织架构"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "org_consult_center");

        // 同名（大小写不同）= 覆盖写入，不算相似
        assert!(find_similar_in_dir(dir, "ORG_CONSULT_CENTER", None).is_empty());
        // 无关主题无候选
        assert!(find_similar_in_dir(dir, "date_parse_recipe", None).is_empty());
    }

    #[test]
    fn duplicate_groups_transitive_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("a.md"), "---\ndescription: 咨询中心组织架构 alpha\n---\n").unwrap();
        fs::write(dir.join("b.md"), "---\ndescription: 咨询中心组织架构 beta\n---\n").unwrap();
        fs::write(dir.join("c.md"), "---\ndescription: 咨询中心组织架构 gamma\n---\n").unwrap();
        fs::write(dir.join("z.md"), "---\ndescription: 完全无关的日期解析排障\n---\n").unwrap();

        let groups = duplicate_groups(dir);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], vec!["a", "b", "c"]);
    }

    #[test]
    fn duplicate_groups_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(duplicate_groups(tmp.path()).is_empty());
    }

    #[test]
    fn boilerplate_descriptions_are_neutralized() {
        // 占位描述（旧版 seed 标记）×3：不应因描述相同而互相关联
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for n in ["a", "b", "c"] {
            fs::write(dir.join(format!("{n}.md")), "---\ndescription: 自动初始化的 OKF 文档\n---\n").unwrap();
        }
        assert!(duplicate_groups(dir).is_empty());

        // 同一实质描述只出现在 2 个文件（真重复对）：信号保留
        let tmp2 = tempfile::tempdir().unwrap();
        let dir2 = tmp2.path();
        fs::write(dir2.join("x.md"), "---\ndescription: 咨询中心组织架构口径说明\n---\n").unwrap();
        fs::write(dir2.join("y.md"), "---\ndescription: 咨询中心组织架构口径说明\n---\n").unwrap();
        fs::write(dir2.join("z.md"), "---\ndescription: 完全无关的描述\n---\n").unwrap();
        let groups = duplicate_groups(dir2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], vec!["x", "y"]);
    }
}
