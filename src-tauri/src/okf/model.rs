//! OKF 数据模型：作用域、类别、frontmatter 值对象、条目与状态枚举。

use std::collections::HashMap;

/// 知识作用域。concepts/users 默认全局；tables/views/sources/recipes 工作区。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    Workspace,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "全局",
            Scope::Workspace => "工作区",
        }
    }
}

/// 知识类别。决定文件子目录、frontmatter type、默认作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Concept,
    User,
    Table,
    View,
    Source,
    Recipe,
}

impl Category {
    /// 默认作用域（concepts/users 全局，其余工作区）。
    pub fn scope(self) -> Scope {
        match self {
            Category::Concept | Category::User => Scope::Global,
            _ => Scope::Workspace,
        }
    }
    /// 在 OKF 根下的子目录。
    pub fn dir(self) -> &'static str {
        match self {
            Category::Concept => "concepts",
            Category::User => "users",
            Category::Table => "tables",
            Category::View => "views",
            Category::Source => "sources",
            Category::Recipe => "pipelines/specific",
        }
    }
    /// frontmatter `type` 字段值。
    pub fn doc_type(self) -> &'static str {
        match self {
            Category::Concept => "Business Concept",
            Category::User => "User Profile",
            Category::Table => "DuckDB Table",
            Category::View => "DuckDB View",
            Category::Source => "Data Source",
            Category::Recipe => "Recipe",
        }
    }
    /// 从工具层字符串解析（容错：未知 → None）。
    pub fn from_str(s: &str) -> Option<Category> {
        match s.trim() {
            "concepts" => Some(Category::Concept),
            "users" | "users/default" => Some(Category::User),
            "tables" => Some(Category::Table),
            "views" => Some(Category::View),
            "sources" => Some(Category::Source),
            "pipelines/specific" | "pipelines" => Some(Category::Recipe),
            _ => None,
        }
    }
}

/// 一张表的列信息（物理画像用）。`(字段名, 物理类型, 是否允许空)`。
pub type ColumnInfo = (String, String, bool);

/// 表探索状态（真源在 table_registry；此处仅作 catalog 渲染的枚举视图）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableStatus {
    Available,
    UnavailablePermanent,
    UnavailableTemporary,
    Unknown,
}
impl TableStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "available" => Self::Available,
            "unavailable_permanent" => Self::UnavailablePermanent,
            "unavailable_temporary" => Self::UnavailableTemporary,
            _ => Self::Unknown,
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Available => "✅",
            Self::UnavailablePermanent => "❌",
            Self::UnavailableTemporary => "⚠️",
            Self::Unknown => "❓",
        }
    }
}

/// 列语义（column_semantics 的返回）：(业务标题, 列名→释义, 关联关系)。
/// 旧类型，step3 将替换为 TableSemantics。
pub type ColumnSemantics = (Option<String>, HashMap<String, String>, Vec<String>);

// ===========================================================================
// 关联关系结构化模型（step3 parser 将使用）
// ===========================================================================

/// 关联方向。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// `→` 单向引用
    OneWay,
    /// `↔` 双向
    TwoWay,
}

impl Direction {
    #[allow(dead_code)]
    pub fn from_arrow(s: &str) -> Option<Self> {
        match s.trim() {
            "→" | "->" => Some(Self::OneWay),
            "↔" | "<->" => Some(Self::TwoWay),
            _ => None,
        }
    }
    #[allow(dead_code)]
    pub fn to_arrow(self) -> &'static str {
        match self {
            Self::OneWay => "→",
            Self::TwoWay => "↔",
        }
    }
}

/// 关联基数。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    OneToOne,
    OneToMany,
    ManyToOne,
    ManyToMany,
}

impl Cardinality {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "1:1" => Some(Self::OneToOne),
            "1:N" => Some(Self::OneToMany),
            "N:1" => Some(Self::ManyToOne),
            "N:M" => Some(Self::ManyToMany),
            _ => None,
        }
    }
    #[allow(dead_code)]
    pub fn to_str(self) -> &'static str {
        match self {
            Self::OneToOne => "1:1",
            Self::OneToMany => "1:N",
            Self::ManyToOne => "N:1",
            Self::ManyToMany => "N:M",
        }
    }
}

/// 一条结构化关联关系。
/// 格式：`- \`local_col\` <方向> [[target_table]].\`target_col\` (<基数>) <描述>`
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub local_col: String,
    pub direction: Direction,
    pub target_table: String,
    pub target_col: String,
    pub cardinality: Cardinality,
    pub description: Option<String>,
}

/// 一列的语义（字段 Schema 表的一行）。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSemantic {
    pub name: String,
    pub ty: String,
    pub comment: String,
    pub constraint: String,
}

/// 表/视图语义解析结果（step3 替代 ColumnSemantics）。
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct TableSemantics {
    pub title: Option<String>,
    pub columns: Vec<ColumnSemantic>,
    pub relations: Vec<Relation>,
}

/// 读取结果。
#[derive(Debug, Clone)]
pub struct OkfReadOutcome {
    pub scope: Scope,
    pub file_path: std::path::PathBuf,
    pub content: String,
}

/// 写入结果。
#[derive(Debug, Clone)]
pub struct OkfWriteOutcome {
    pub scope: Scope,
    pub file_path: std::path::PathBuf,
    pub created: bool,
}

/// 搜索命中。`rel_path` 形如 `[全局] concepts/foo.md`（scope 已嵌入文本）。
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub rel_path: String,
    pub preview: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_parse() {
        assert_eq!(Direction::from_arrow("→"), Some(Direction::OneWay));
        assert_eq!(Direction::from_arrow("->"), Some(Direction::OneWay));
        assert_eq!(Direction::from_arrow("↔"), Some(Direction::TwoWay));
        assert_eq!(Direction::from_arrow("<->"), Some(Direction::TwoWay));
        assert_eq!(Direction::from_arrow("??"), None);
        assert_eq!(Direction::OneWay.to_arrow(), "→");
        assert_eq!(Direction::TwoWay.to_arrow(), "↔");
    }

    #[test]
    fn cardinality_parse() {
        assert_eq!(Cardinality::from_str("1:1"), Some(Cardinality::OneToOne));
        assert_eq!(Cardinality::from_str("1:N"), Some(Cardinality::OneToMany));
        assert_eq!(Cardinality::from_str("N:1"), Some(Cardinality::ManyToOne));
        assert_eq!(Cardinality::from_str("N:M"), Some(Cardinality::ManyToMany));
        assert_eq!(Cardinality::from_str("??"), None);
        assert_eq!(Cardinality::ManyToOne.to_str(), "N:1");
    }

    #[test]
    fn category_scope_and_paths() {
        assert_eq!(Category::Concept.scope(), Scope::Global);
        assert_eq!(Category::User.scope(), Scope::Global);
        assert_eq!(Category::Table.scope(), Scope::Workspace);
        assert_eq!(Category::Recipe.scope(), Scope::Workspace);
        assert_eq!(Category::Recipe.dir(), "pipelines/specific");
        assert_eq!(Category::View.doc_type(), "DuckDB View");
    }

    #[test]
    fn category_from_str_roundtrip() {
        assert_eq!(Category::from_str("concepts"), Some(Category::Concept));
        assert_eq!(Category::from_str("users/default"), Some(Category::User));
        assert_eq!(Category::from_str("pipelines/specific"), Some(Category::Recipe));
        assert_eq!(Category::from_str("  views "), Some(Category::View));
        assert_eq!(Category::from_str("nope"), None);
    }

    #[test]
    fn scope_labels() {
        assert_eq!(Scope::Global.label(), "全局");
        assert_eq!(Scope::Workspace.label(), "工作区");
    }

    #[test]
    fn table_status_icons() {
        assert_eq!(TableStatus::from_str("available").icon(), "✅");
        assert_eq!(TableStatus::from_str("unavailable_permanent").icon(), "❌");
        assert_eq!(TableStatus::from_str("unavailable_temporary").icon(), "⚠️");
        assert_eq!(TableStatus::from_str("garbage").icon(), "❓");
    }
}
