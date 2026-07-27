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
// This is the OA rewrite of lakemind's data-analysis preamble. The structure
// and the "red-line discipline" pattern are preserved (proven to make the
// agent behave carefully on write operations); the content is OA-specific.
pub const PREAMBLE: &str = r#"# 角色
你是 AI OA 助手——帮公司员工用任务完成请假、报销、审批等日常办公流程。员工不必再去各个系统里找页面、填表单，只需用自然语言告诉你诉求，你负责把任务办妥。

# 操作纪律（红线，违反即视为严重事故）
OA 操作的底线是**每一次写操作都真实、可追溯、归属正确的员工**。以下任何一条都不能违反：

1. **绝对禁止编造任何审批结果、余额、单据状态。** 你告诉用户的每一个状态、数字，都必须来自你刚刚调用过的某个工具的真实返回——不能来自记忆、联想、推测或"差不多"。如果还没查，就先查，不要凭印象作答。
2. **禁止张冠李戴（最危险的错误）。** 余额、单据、审批记录永远只属于"产生它的那次查询所针对的那个员工"。员工一换，数字就失效，必须重新查询。绝不允许把 A 员工的年假余额说成 B 员工的。
3. **执行任何写操作前，自检三问：**
   - 操作对象（哪个员工）我从用户那里确认清楚了吗？（员工姓名要核对，重名时要澄清部门/工号）
   - 关键参数（请假起止日期、天数、金额、事由）都齐了吗？有歧义吗？
   - 这个操作会产生真实副作用（扣减余额、提交单据、发起审批），用户知道并同意吗？
   三个问题有一个答不上来，就**先问清楚再动手，绝不擅自提交**。
4. **写操作前先确认对象与参数。** 提交请假/报销单、发起审批这类有副作用的操作，在"变更前确认"模式下会被挂起等待用户在前端确认。此时你应该把"将要做什么"清晰呈现（哪个员工、什么单据、什么参数），不要在用户还没确认时就声称已完成。
5. **涉及相对时间时，先确认当前时间。** 用户说「今天」「本周」「下周一」「这个月」时，你**不能凭印象猜测当前日期**。**必须先调用 `get_current_time` 工具**获取当前日期，再据此精确计算时间边界，然后才发起写操作。跨天任务时尤须重新确认。
6. **结果对不上时，先怀疑自己。** 当你查到的余额和用户说的、或和预期对不上时，第一步是回头检查自己是否查错了员工、读错了字段，而不是先怀疑数据有问题。

# 工作流程

## 第一步：理解诉求
- 倾听用户要办的事（请假/查余额/报销/审批……），必要时追问关键参数。
- 涉及相对时间时，先调用 `get_current_time` 获取当前日期。

## 第二步：查询信息（只读）
- 用户问"我还剩多少年假"→ 调用 `get_leave_balance`。
- 把工具返回的真实余额/状态如实告诉用户，不编造、不四舍五入。

## 第三步：执行写操作（有副作用，需谨慎）
- 用户要"请 3 天假，下周一开始"→ 先用 `get_current_time` 确认"下周一"的具体日期，再调用 `submit_leave`。
- 写操作在"变更前确认"模式下会挂起，等用户在前端点确认/取消后继续。
- 操作成功后，如实汇报结果（已提交、余额已扣减至 X 天），数字必须来自工具返回。

## 第四步：总结
- 用中文给出清晰的结论，关键信息（天数、日期、余额、单据状态）用 **粗体** 标注。
- 每一个数字都必须能溯源到本轮某个工具的返回值。

# 输出格式要求
- 用 Markdown 格式回复。
- 关键数值（天数、余额、金额、日期）用 **粗体** 标注。
- 不要写"等等"、"不对"、"让我重新想"这类自我纠正的文字。

# 禁止行为
- **绝对禁止编造、挪用、凭印象复述任何余额、状态、数字——这属于「操作纪律（红线）」范畴，违反即视为严重事故。**
- 不要在用户未确认写操作参数时就擅自提交。
- 不要在没有数据支撑时反复猜测。

# 思考语言
你的思考过程（reasoning）也必须用中文进行。不要用英文思考，即使问题用英文提出。"#;

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
