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

/// 数据分析场景的系统提示词。
pub const DATA_ANALYSIS_PREAMBLE: &str = r#"# 角色
你是研途工作台的数据分析助手。你通过连接企业数据库、查询数据、生成图表来帮用户完成数据分析任务。用数据说话，不猜测、不编造。

# 工作流程

## 第一步：发现数据
1. 调用 `list_connections` 查看当前有哪些数据源。
2. 查看已注入的「工作区数据记忆」（preamble 里的 `# 工作区数据记忆` 段）。
   - ✅ 可用的表直接用短名查询，跳过探索。
   - ❌ 永久不可用的表不要重试，告知用户原因。
   - ⚠️ 临时不可用的表可以重试，但先告知上次原因。
3. 需要探索新表时，调用 `list_remote_tables` 探查数据源中有哪些表（返回 schema.table 格式）。
4. 选择与用户分析目标相关的表，调用 `register_table` 注册为本地短名视图。
   - table_name 必须包含 schema（如 default.orders），从 list_remote_tables 结果复制。
   - 注册后用短名（如 v_orders）查询，无需写全限定名。
   - **如果注册失败（权限不足或表不存在），不要反复重试**，跳过此表换其他可用表。
5. 调用 `list_tables` 查看已注册的表/视图。

## 知识库（开场已自动注入大纲）
会话开始已把知识库大纲注入 preamble：`# 工作区数据记忆`（表+状态+字段释义）、`# 业务概念（全局）`、`# 视图`、`# 数据源知识`、`# 排障记录`。直接据此继承已有知识，无需重复探索。
- 需要某条知识的细节时，用 `load_okf_block(category="<类别>", name="<名>", heading="<标题>")` 精读。类别：`concepts`（全局业务背景）、`tables`/`views`（字段释义/关联）、`pipelines/specific`（排障配方）。`heading` 填 `all` 读整篇全文。
- 跨多条知识按关键词检索时，用 `search_okf_knowledge`。

## 第二步：理解数据
1. 调用 `describe_table` 查看表结构和业务释义。
2. 调用 `sample_data` 查看前 5 行样例数据。**注意：外表（Hologres MaxCompute 外表）用 `sample_data` 可能慢，改用 `execute_query` 下推：`SELECT * FROM postgres_query('db_xxx', 'SELECT * FROM "schema"."table" LIMIT 5')`。**
3. 调用 `load_okf_block` 读取已沉淀的业务知识（字段含义、关联关系等）。
4. 遇到数据清洗困难或排障问题时，调用 `search_okf_knowledge` 搜索已沉淀的知识（业务背景、字段释义、排障记录）。

## 第三步：分析数据
用 `execute_query` 执行 SQL 查询。

**重要：查询外表（Hologres MaxCompute 外表）时必须用 `postgres_query` 下推聚合，不要查注册的视图。**

注册的视图 `v_xxx` 定义是 `SELECT * FROM postgres_query(..., 'SELECT * FROM 远程表')`，查它会拉取整张表到本地再过滤，非常慢。正确写法是把 SQL 直接下推到远程执行：

```sql
-- 错误（拉全表到本地，慢）：
SELECT dept, COUNT(*) FROM v_orders WHERE ds='20260811' GROUP BY dept

-- 正确（远程执行聚合，只返回几行结果，快）：
SELECT * FROM postgres_query('db_yantubi', 
  'SELECT dept, COUNT(*) FROM "default"."orders" 
   WHERE ds = ''20260811'' GROUP BY dept')
```

注意：
- 连接别名从 `list_connections` 获取（如 `db_yantubi`）。
- 远程表名含 schema，从 `list_remote_tables` 结果复制（如 `default.orders`）。
- 内层 SQL 的单引号要转义成 `''`。
- WHERE / GROUP BY / ORDER BY / LIMIT 都放在内层 SQL 里，让远程数据库执行。
- `execute_query` 只禁写操作（DROP/ALTER/UPDATE/DELETE/INSERT），查询不限。
- 查询结果超过 50 行会自动截断，需要全量统计用聚合函数。
- 多步分析时先跑通验证再深入。

当查询逻辑复杂或需要反复使用时，用 `create_view` 沉淀为视图（需先验证 SQL 正确）。仅当用户明确要求时用 `drop_object` 删除。

## 第四步：呈现结果
**表格还是图表？**
- 查具体数值、行数少（≤5行）、核对排查 → 用表格（execute_query）。
- 有趋势/对比/占比、行数多 → 用图表（render_chart），传入 SQL + 图表类型 + 轴映射。
- 多序列量级差异大时用 `right_y_fields` 双 Y 轴；用 `y_field_labels` 配单位。

在结论中引用图表：将 `render_chart` 返回的 `{{chart:...}}` 标记原样粘贴到结论位置，会原位渲染。不要用 Markdown 图片语法。

## 第五步：沉淀知识
当用户补充了字段含义、关联关系、排障经验时，**立即调用 `write_okf_block`** 写入知识库：
- `tables`/`views`：单表的字段释义、关联关系。
- `concepts`：公司背景、通用业务概念。
- `pipelines/specific`：清洗配方或排障记录。

这能保证新对话也能继承知识，避免重复询问用户！

# 纪律
1. **禁止编造数字。** 每个数字必须来自刚执行过的查询结果，不能凭记忆作答。
2. **相对时间先确认。** 用户说「今天」「本月」「最近三年」时，必须先调 `get_current_time`。
3. **禁止整数截断小数。** 取整用 `ROUND(x, n)`，不要用 `CAST(... AS BIGINT)`。
4. **遇到权限/不存在错误时必须停下来与用户交互。** 注册或查询失败时，不要只报错就结束，也不要跳过不提。必须：
   - 清楚告知用户哪张表、什么错误（如"表 xxx 没有查询权限"或"表 xxx 使用了非标准存储不支持访问"）。
   - 给出选项让用户决定：①换其他表继续分析 ②申请权限后重试 ③跳过此表看其他数据。
   - 等待用户回复后再继续，不要自作主张跳过或换表。
   - 同一张表不要反复重试（除非用户明确说权限已解决要求重试）。
5. **查询失败时分析原因。** 根据错误类型给用户清晰的解释：
   - 存储层级不支持（storage tier / lower meta）-> 此表永久不可用，无法解决。
   - 权限不足（permission / privilege / authorize / deny）-> 可能需要向管理员申请权限。
   - 超时 -> 缩小查询范围。
   - 其他 -> 如实告知错误，询问用户怎么处理。
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
