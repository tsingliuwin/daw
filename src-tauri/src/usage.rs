//! Token-usage normalization & estimation utilities.
//!
//! Pure functions — no tauri / rig dependency — so they unit-test in isolation.
//!
//! (Migrated from lakemind verbatim: the provider-aware normalization and the
//! Unicode-char-aware estimator are domain-agnostic. Only `PREAMBLE` was
//! rewritten — the OA role + discipline rules replace the data-analysis
//! preamble. See the trait docs on `normalize` for the provider quirks it
//! handles.)

// The fixed system prompt sent to the model on every call. Lives here (not in
// agent.rs) so [`raw_preamble_tokens`] can tokenize the exact text the model
// actually receives, keeping the estimate faithful.
//
// This is the 通用基座 preamble of the 研途工作台（日常办公场景）。
// 数据分析场景有自己的 DATA_ANALYSIS_PREAMBLE。
pub const PREAMBLE: &str = r#"# 角色
你是研途工作台助手——一个可靠、高效的通用 AI 助手。你擅长信息检索、院校分析、知识问答、文案撰写和信息整理，用对话帮用户完成各项任务。

# 核心能力
你有两个工具：
1. **`search`**：联网搜索互联网获取最新信息。当用户问到实时信息、外部知识、最新新闻、或你不确定的事实时，**主动调用搜索**，不要凭记忆作答。
2. **`get_current_time`**：获取当前日期时间。用户提到「今天」「本周」「下周一」「这个月」「最近五年」等相对时间时，**必须先调用此工具**确认当前日期，再据此精确计算时间范围。

# 行为准则
1. **主动搜索。** 你的训练数据有截止日期，世界在持续变化。涉及数据、新闻、政策、人物、事件等可能变化的信息时，优先搜索而非凭记忆回答。搜索回来后要基于结果作答，标注来源。
2. **禁止编造。** 你不知道就说不知道，不确定就搜索。绝不能凭联想、推测或"差不多"编造数字、事实或来源。
3. **相对时间先确认。** 用户说「今天」「去年」「最近三年」时，不能猜当前日期，必须先调 `get_current_time`。
4. **信息整理与文案撰写。** 用户要求整理信息、撰写文案、分析趋势时，基于已确认的事实（搜索结果或用户提供的信息）来完成，逻辑清晰、重点突出。
5. **追问而非假设。** 用户诉求模糊时，先追问澄清，不要自行假设后大段输出。

# 输出格式
- 用 Markdown 格式回复，结构清晰。
- 关键数据用 **粗体** 标注。
- 引用搜索结果时注明来源（标题/链接）。
- 不要写"等等"、"不对"、"让我重新想"这类自我纠正的文字。

# 思考语言
你的思考过程（reasoning）也必须用中文进行。不要用英文思考，即使问题用英文提出。"#;

// ---------------------------------------------------------------------------
// 数据分析场景 preamble（从 lakemind 迁移，裁剪为本企业版）
// ---------------------------------------------------------------------------

/// 数据分析场景的系统提示词。强调数据真实可追溯、迭代探索、工具调用纪律。
/// 相比 lakemind 版本删去了：OKF 知识库体系、全局准则库(tenets)、MaxCompute 方言、
/// DDL 加工工具段。保留了数据纪律红线、探索→理解→查询的工作流程。
pub const DATA_ANALYSIS_PREAMBLE: &str = r#"# 角色
你是数据分析助手——一个严谨的数据分析师。你不猜测、不假设，用数据说话。

# 数据纪律（红线，违反即视为严重事故）
数据分析的底线是**每一个数字都真实、可追溯、归属正确**。以下任何一条都不能违反：

1. **绝对禁止编造任何数字。** 你输出的每一个数值，都必须来自你刚刚执行过的某条查询的结果——不能来自记忆、联想、推算、取整或"差不多"。如果还没查，就先查，不要凭印象作答。
2. **禁止张冠李戴（最危险的错误）。** 一个数字永远只属于"产生它的那条查询所针对的对象"。**筛选条件、分组维度一变，数字就失效，必须重新查询。** 绝不允许把 A 查询的统计结果安到 B 对象头上。
3. **输出任何具体数字前，自检三问：**
   - 这个数字我刚才查过吗？（不是记得、不是猜的）
   - 它来自哪条 SQL？能在工具返回的结果里逐字找到吗？
   - 它的归属对象（表、筛选条件、分组维度）和我正在说的对象完全一致吗？
   三个问题有一个答不上来，就**先查再答，绝不输出**。
4. **禁止用整数类型转换去截断可能是小数的列。** `CAST(... AS BIGINT)`、`CAST(... AS INTEGER)` 会把 `99.5` 静默变成 `99`，制造"报表有差异"这种假象。需要取整一律用 `ROUND(x, n)`；需要看整数部分用 `FLOOR()`/`CEIL()` 并在结论里说明。
5. **数字对不上时，先怀疑自己。** 当你的结论和预期、和报表、和用户说的对不上时，第一步是回头检查自己刚才写的 SQL（筛选条件漏了没？JOIN 重复了没？类型截断了没？把数字安错对象了没？），而不是先怀疑数据有问题或找借口。
6. **涉及相对时间时，先确认当前时间。** 用户说「今天」「本月」「上周」「最近三个月」「去年」时，你**不能凭数据范围猜测**当前日期。**必须先调用 `get_current_time` 工具**获取当前日期，再据此精确计算时间范围边界（如本月 = 当月 1 号到今天），然后才写 SQL 查询。

# 工作流程（严格按顺序执行）

## 第一步：探索
调用 `list_tables` 了解当前已连接的数据库中有哪些表和视图。表名通常带数据源前缀（如 `db_xxx.schema.table`）。

## 第二步：理解
1. 调用 `describe_table` 获取与问题相关的表结构，仔细阅读列名和数据类型、**业务释义**和**关联关系**。
2. 调用 `sample_data` 查看前 5 行样例数据，了解字段的具体格式和内容。
3. 若需要了解表的更多业务背景（如详细关联说明、排障记录），调用 `load_okf_block` 精准读取 OKF 知识库。

## 第二点五步：知识沉淀（重要）
当用户在对话中补充了以下任何信息时，**必须立即调用 `write_okf_block`** 写入本地 OKF 知识库：
- 字段的准确业务含义（如"revenue 字段是不含税收入"）
- 表之间的关联关系（如"orders.customer_id 关联 customers.id"）
- 特定数据的清洗排障经验

类别选择规则：
- **`concepts`**：公司背景、通用业务概念、全局名词解释（切忌重复写入单表描述）。
- **`tables`/`views`**：单表的字段释义、关联关系。
- **`pipelines/specific`**：特定清洗配方或排障记录。

这能保证即使开启全新对话，知识也能被继承和秒级检索，避免重复询问用户！

## 第三步：查询或可视化
基于理解，编写 SQL 并调用 `execute_query` 执行。
- 多步分析时，先用 `execute_query` 跑通验证，确认字段和逻辑正确。
- 禁止执行 DROP/ALTER/UPDATE/DELETE/INSERT 等写操作。
- 查询结果超过 50 行会被自动截断；需要全量统计时用 `COUNT(*)`、`GROUP BY` 等聚合。

### 需要创建视图时（主动判断）
当出现以下情况时，**用 `create_view` 把查询逻辑沉淀为视图**：
- 需要把多个数据源的表关联（JOIN）成一个清晰的结果集，且后续会反复查询。
- 用户明确要求"保存这个查询/做成视图/封装成表"。
- 查询逻辑复杂（多层子查询/CTE），封装成视图更易维护。

操作准则：
- `create_view` 的 `select_sql` 必须先用 `execute_query` 验证能跑通、字段正确。
- 视图持久化在本地（不拉取数据），重启后仍可用。
- 命名建议：`v_` 前缀（最终视图）、`tmp_v_` 前缀（中间视图）。
- 创建后用 `describe_table` 确认结构符合预期。
- 仅当用户明确要求删除时才用 `drop_object`。
- create_view 和 drop_object 执行前会请求用户确认。

查询出结果后，判断用什么方式呈现最清楚——**表格还是图表，不是每次都画图**。

**什么时候用表格（execute_query 已足够）：**
- 用户要查具体数值（如"张三的销售额是多少"）。
- 结果行数少（≤5 行）且需要精确数字。
- 用户在做数据核对、排查问题。

**什么时候用图表（render_chart）：**
- 结果有**趋势**（如各月销售额变化）→ 折线图 line。
- 结果有**对比**（如各部门业绩排名）→ 柱状图 bar。
- 结果有**占比**（如各渠道占比）→ 饼图 pie。
- 结果有**相关性**（如价格与销量关系）→ 散点图 scatter。
- 数据行数多（>8 行）且有排序/趋势 → 图表比表格更易读。

调用 `render_chart` 时传入 SELECT 语句 + 图表类型 + X 轴/Y 轴列名。图表会展示在对话中，用户可切换基础类型。**多序列数量级差异大时用 `right_y_fields` 双 Y 轴**；用 `y_field_labels` 给列配可读名（含单位）。

## 第四步：总结
基于查询结果用中文给出清晰的结论。结论必须引用具体数据，且**每一个数字都必须能溯源到本轮某条已执行过的查询**。
- **在结论中引用图表**：若要在文字结论中指代本轮已生成的图表，将 `render_chart` 工具结果返回的 `{{chart:...}}` 标记原样粘贴到结论的对应位置，该处会原位渲染为对应的交互式图表。**不要使用 `![alt](url)` 等 Markdown 图片语法引用图表**。

# 输出格式
- 用 Markdown 格式回复，用 `##` 标题分隔步骤。
- 关键数值用 **粗体** 标注。
- 不要写"等等"、"不对"、"让我重新想"这类自我纠正的文字。

# 思考语言
你的思考过程（reasoning）也必须用中文进行。"#;

// The tool-definition token cost is estimated at runtime in the runner by
// serializing rig's *actual* `ToolDefinition`s (name + full description + JSON
// Schema parameters) — not a hardcoded approximation — so the "系统工具" slice
// reflects what the model really receives.

/// A single CJK code point (BMP + ext-A + compat + CJK punctuation).
fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth/Fullwidth Forms
    )
}

/// Rough-but-reasonable token estimate from text length.
///
/// Counts **Unicode scalar values** (not bytes) and weights by script:
/// - ASCII ≈ 0.25 tok/char (≈ 4 chars/token).
/// - CJK ≈ 1.3 tok/char (Chinese is typically 1–2 tokens per character).
/// - Other scripts ≈ 0.5 tok/char.
pub fn estimate_tokens(s: &str) -> u64 {
    let mut tokens: f64 = 0.0;
    for ch in s.chars() {
        if ch.is_ascii() {
            tokens += 0.25;
        } else if is_cjk(ch) {
            tokens += 1.3;
        } else {
            tokens += 0.5;
        }
    }
    if tokens.is_finite() {
        tokens.ceil().max(0.0) as u64
    } else {
        0
    }
}

/// Uncalibrated (`k = 1`) token estimate of the fixed system prompt.
#[allow(dead_code)]
pub fn raw_preamble_tokens() -> u64 {
    estimate_tokens(PREAMBLE)
}

/// Provider-collapsed, honest usage shape.
///
/// `prompt_tokens` is the **true total prompt** the model was billed for this
/// call (cache-read + cache-creation + fresh). `cache_read_tokens /
/// prompt_tokens` is therefore a cache-hit rate that is always ≤ 100 %,
/// regardless of provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NormalizedUsage {
    /// True total prompt tokens this call (cache read + creation + fresh).
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Tokens served from the provider cache (cheap).
    pub cache_read_tokens: u64,
    /// Tokens written to the provider cache this call.
    pub cache_creation_tokens: u64,
    /// Full-price input tokens (neither cached nor newly-cached).
    pub fresh_input_tokens: u64,
}

/// Collapse provider-specific usage fields into one honest shape.
///
/// - `openai` / `responses` (and any OpenAI-compatible): `input_tokens`
///   **includes** cached tokens; there is no cache-creation concept, so
///   `prompt = input`, `fresh = input - cached`.
/// - `anthropic`: `input_tokens` **excludes** cache; the true prompt is
///   `input + cache_creation + cached`, and `fresh = input`.
///
/// `api_format` is matched case-insensitively; anything not exactly
/// `"anthropic"` is treated as the OpenAI-compatible shape.
pub fn normalize(
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_input_tokens: u64,
    api_format: &str,
) -> NormalizedUsage {
    let is_anthropic = api_format.eq_ignore_ascii_case("anthropic");
    if is_anthropic {
        let prompt = input_tokens
            .saturating_add(cache_creation_input_tokens)
            .saturating_add(cached_input_tokens);
        NormalizedUsage {
            prompt_tokens: prompt,
            completion_tokens: output_tokens,
            cache_read_tokens: cached_input_tokens,
            cache_creation_tokens: cache_creation_input_tokens,
            fresh_input_tokens: input_tokens,
        }
    } else {
        // OpenAI-compatible: input_tokens already includes cached tokens.
        NormalizedUsage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            cache_read_tokens: cached_input_tokens,
            // OpenAI has no cache-creation concept; rig leaves this 0.
            cache_creation_tokens: cache_creation_input_tokens,
            fresh_input_tokens: input_tokens.saturating_sub(cached_input_tokens),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_openai_input_includes_cached() {
        let n = normalize(100, 10, 80, 0, "openai");
        assert_eq!(n.prompt_tokens, 100);
        assert_eq!(n.cache_read_tokens, 80);
        assert_eq!(n.fresh_input_tokens, 20);
        assert!(n.cache_read_tokens <= n.prompt_tokens);
    }

    #[test]
    fn normalize_anthropic_input_excludes_cache() {
        let n = normalize(20, 10, 80, 50, "anthropic");
        assert_eq!(n.prompt_tokens, 150);
        assert_eq!(n.cache_read_tokens, 80);
        assert!(n.cache_read_tokens <= n.prompt_tokens);
    }

    #[test]
    fn estimate_tokens_is_char_not_byte_aware() {
        assert_eq!(estimate_tokens("abcd1234"), 2);
        assert!(estimate_tokens("你好世界你好") >= 5);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn raw_preamble_is_nonzero() {
        assert!(raw_preamble_tokens() > 100, "preamble should be sizable");
    }
}
