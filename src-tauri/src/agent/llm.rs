//! One-shot LLM completion (no tools, no window events).
//!
//! (Migrated verbatim from lakemind. Used for background LLM tasks — e.g.
//! generating a human-readable title for a leave request. Mirrors the provider-
//! client construction in [`crate::agent::runner::run_agent_chat_stream`] but
//! stripped down.)

use rig_core::{
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    completion::Message,
    streaming::{StreamedAssistantContent, StreamingChat},
};

use super::config::{get_provider_for_model, sanitize_endpoint};

/// One-shot LLM completion: stream the model's reply but gather all text locally
/// (no window events, no tools). Returns the concatenated assistant text.
///
/// M2 entry point: used for background LLM tasks (leave-request title
/// generation, conversation archiving). Unused in M1.
#[allow(dead_code)]
pub(crate) async fn complete_one_shot(
    prompt: &str,
    model_id: &str,
    provider_id: Option<&str>,
) -> Result<String, String> {
    let provider = get_provider_for_model(model_id, provider_id)?;
    let format = provider.api_format.to_lowercase();
    let max_tokens: u64 = 64;

    if format == "openai" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(&provider.api_key)
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
        let agent = client.completions_api().agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    if format == "responses" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(&provider.api_key)
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
        let agent = client.agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    if format == "anthropic" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::anthropic::Client =
            rig_core::providers::anthropic::Client::builder()
                .api_key(provider.api_key.clone())
                .base_url(&base_url)
                .build()
                .map_err(|e| format!("构建 Anthropic 客户端失败: {e}"))?;
        let agent = client.agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    Err(format!("不支持的 API 格式: {}", provider.api_format))
}

/// Collect all assistant text deltas from a rig multi-turn stream into a single
/// string.
async fn collect_stream_text<R>(
    mut stream: impl futures_util::Stream<Item = Result<MultiTurnStreamItem<R>, rig_core::agent::StreamingError>> + Unpin,
) -> Result<String, String> {
    use futures_util::StreamExt;
    let mut out = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(t))) => {
                out.push_str(&t.text);
            }
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(out)
}

/// Connection test for an LLM provider: send a minimal prompt against the given
/// ad-hoc config and return Ok if the model replied. Tests exactly the values
/// shown in the settings form, so unsaved edits are reflected immediately.
pub(crate) async fn test_connection(
    endpoint: &str,
    api_key: &str,
    api_format: &str,
    model_id: &str,
) -> Result<(), String> {
    let format = api_format.to_lowercase();
    let base_url = sanitize_endpoint(endpoint);
    let max_tokens: u64 = 8;
    let prompt = "hi";

    let res = build_and_run_test(&format, &base_url, api_key, model_id, max_tokens, prompt).await;
    res.map(|_| ()).map_err(|e| friendly_llm_err(&e))
}

async fn build_and_run_test(
    format: &str,
    base_url: &str,
    api_key: &str,
    model_id: &str,
    max_tokens: u64,
    prompt: &str,
) -> Result<String, String> {
    if format == "openai" {
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
        let agent = client.completions_api().agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    if format == "responses" {
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
        let agent = client.agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    if format == "anthropic" {
        let client: rig_core::providers::anthropic::Client =
            rig_core::providers::anthropic::Client::builder()
                .api_key(api_key.to_string())
                .base_url(base_url)
                .build()
                .map_err(|e| format!("构建 Anthropic 客户端失败: {e}"))?;
        let agent = client.agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    Err(format!("不支持的 API 格式: {format}"))
}

/// Translate a raw LLM provider error string into a clear, actionable Chinese
/// message for the settings "test connection" UI.
pub(crate) fn friendly_llm_err(msg: &str) -> String {
    let m = msg.to_lowercase();

    // 1. Quota / balance exhausted (may carry 429 — check before the auth rule).
    const QUOTA: &[&str] = &[
        "insufficient_quota", "insufficient quota", "insufficient balance",
        "quota exhausted", "quota_exhausted", "exceeded your current quota",
        "credit_balance", "credit balance", "balance is too low", "balance too low",
        "no credit", "out of credit", "余额不足", "额度已用尽", "额度不足", "欠费",
    ];
    if QUOTA.iter().any(|k| m.contains(k)) {
        return format!(
            "该账号额度/余额已用尽，请到服务商后台充值后再试。\n\n原始错误：{msg}"
        );
    }

    // 2. Authentication / authorization failure. A wrong API *format* typically
    //    surfaces here too.
    if m.contains("status code 401") || m.contains("status code 403")
        || m.contains("invalid_api_key") || m.contains("missing_api_key")
        || m.contains("authentication") || m.contains("unauthorized") || m.contains("forbidden")
        || m.contains("invalid api key")
    {
        return format!(
            "鉴权失败：API Key 无效，或 API 格式（openai/anthropic/responses）与该服务商不匹配，请检查这两项。\n\n原始错误：{msg}"
        );
    }

    // 3. Not found — wrong base URL path or model id.
    if m.contains("status code 404") || m.contains("not_found") || m.contains("not found")
        || m.contains("model_not_found") || m.contains("does not exist")
    {
        return format!(
            "未找到该模型：请检查 Base URL 路径和模型 ID 是否正确。\n\n原始错误：{msg}"
        );
    }

    // 4. Transient rate limit (not quota) — retryable.
    if m.contains("status code 429") || m.contains("too many requests")
        || m.contains("rate_limit") || m.contains("overloaded")
    {
        return format!(
            "请求过于频繁被限流，请等待几秒后重试。\n\n原始错误：{msg}"
        );
    }

    // 5. Server-side error (5xx) — provider outage.
    for code in ["500", "502", "503", "504"] {
        if m.contains(&format!("status code {code}")) {
            return format!(
                "模型服务端暂时不可用（HTTP {code}），可能是服务过载或维护中，请稍后重试或更换模型/服务商。\n\n原始错误：{msg}"
            );
        }
    }

    // 6. Fallback: surface the raw message.
    msg.to_string()
}

#[cfg(test)]
mod tests {
    use super::friendly_llm_err;

    #[test]
    fn auth_failure_hint_checks_key_and_format() {
        let msg = "Invalid status code 401 Unauthorized with message: invalid_api_key";
        let out = friendly_llm_err(msg);
        assert!(out.contains("鉴权失败"));
        assert!(out.contains("API 格式"));
    }

    #[test]
    fn model_not_found_hint_checks_url_and_id() {
        let msg = "Invalid status code 404 with message: model_not_found: model 'foo' does not exist";
        let out = friendly_llm_err(msg);
        assert!(out.contains("未找到该模型"));
    }

    #[test]
    fn quota_exhausted_hint_bills() {
        let msg = "Invalid status code 429 with message: {\"error\":{\"code\":\"insufficient_quota\"}}";
        let out = friendly_llm_err(msg);
        assert!(out.contains("额度"));
        assert!(out.contains("充值"));
    }

    #[test]
    fn unknown_error_keeps_raw_message() {
        let msg = "Invalid status code 418 with message: I'm a teapot";
        assert_eq!(friendly_llm_err(msg), msg);
    }
}
