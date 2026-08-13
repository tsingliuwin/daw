//! Core streaming runner: drive the rig multi-turn stream, map its items to
//! frontend events, and assemble the full agent (基座内置工具 + 各 skill 工具).
//!
//! The streaming loop, rate-limit retry, usage accounting, abort handling,
//! and provider-client construction are domain-agnostic and preserved verbatim.
//! Tools come from the skill registry (基座内置 + 已注册 skill).

use serde_json::json;
use rig_core::{
    agent::{MultiTurnStreamItem, StreamingError},
    client::CompletionClient,
    completion::Message,
    streaming::{StreamingChat, StreamedAssistantContent},
    tool::Tool,
};

use super::config::{get_provider_for_model, sanitize_endpoint};
use super::events::{
    emit_delta, emit_event, emit_usage_estimate, emit_usage_real, emit_usage_run_summary,
};
use super::wire::{ChatMessageDto, Segment};
use crate::skill::builtin::GetCurrentTimeTool;
use crate::skill::data_analysis::{create_view::CreateViewTool, describe_table::DescribeTableTool, drop_object::DropObjectTool, execute_query::ExecuteQueryTool, list_connections::ListConnectionsTool, list_remote_tables::ListRemoteTablesTool, list_tables::ListTablesTool, load_okf_block::LoadOkfBlockTool, register_table::RegisterTableTool, render_chart::RenderChartTool, sample_data::SampleDataTool, search_okf_recipes::SearchOkfRecipesTool, write_okf_block::WriteOkfBlockTool};
use crate::skill::search::SearchTool;
use crate::skill::Scenario;
use crate::state::AppState;
use crate::usage::{self, DATA_ANALYSIS_PREAMBLE, PREAMBLE};

/// Rebuild the LLM chat history from persisted messages.
///
/// Legacy messages carry a flat `content` string; new messages carry `segments`.
/// Only visible text reaches the model — reasoning and tool steps are managed
/// by rig within the turn and are not replayed as history.
fn get_message_text(msg: &ChatMessageDto) -> String {
    if let Some(c) = &msg.content {
        return c.clone();
    }
    if let Some(segs) = &msg.segments {
        let mut out = String::new();
        for s in segs {
            if let Segment::Text { text, .. } = s {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        return out;
    }
    String::new()
}

/// Outcome of one streaming run. `RateLimited` is returned only when a 429 /
/// rate-limit error arrives *before* any content was emitted to the frontend,
/// so the caller can rebuild the stream and retry with backoff. Once content
/// has been emitted we can no longer safely retry (the multi-turn state has
/// advanced), so any later error is terminal.
enum RunOutcome {
    Done,
    /// Carries the last rate-limit error string so that when retries are
    /// exhausted the caller can surface *why*.
    RateLimited(String),
}

/// Classify a provider error string into one of three buckets so the runner
/// knows whether retrying is worthwhile.
///
/// rig's `http_client` formats every non-2xx response uniformly as
/// `"Invalid status code {StatusCode} with message: {body}"`. The status code
/// is a stable number, but providers overload 429 to mean two very different
/// things:
///   - **Transient throttling** (TPM/RPM exceeded) — RETRY after backoff.
///   - **Quota/balance exhausted** — DON'T RETRY (just surface the error).
///
/// Both return 429, so the status code alone can't tell them apart. We look at
/// the body wording: quota/balance/credit keywords → `Unretriable` (checked
/// first), otherwise a 429 or throttle keyword → `Retriable`.
#[derive(Debug, PartialEq, Eq)]
enum RateLimitKind {
    Retriable,
    Unretriable,
    No,
}

fn classify_rate_limit_error(msg: &str) -> RateLimitKind {
    let m = msg.to_lowercase();

    // Unretriable: account out of quota / balance / credit. Check BEFORE 429.
    const UNRETRIABLE: &[&str] = &[
        "insufficient_quota", "insufficient quota", "insufficient balance",
        "quota exhausted", "quota_exhausted", "exceeded your current quota",
        "credit_balance", "credit balance", "balance is too low", "balance too low",
        "billing", "payment", "no credit", "out of credit",
        "额度已用尽", "额度不足", "余额不足", "余额已尽", "余额耗尽",
        "充值", "欠费", "计费",
    ];
    if UNRETRIABLE.iter().any(|k| m.contains(k)) {
        return RateLimitKind::Unretriable;
    }

    if m.contains("status code 429") {
        return RateLimitKind::Retriable;
    }

    // Fallback: non-standard gateways expressing throttling via 503 or 200 with
    // a rate-limit body (no 429).
    if m.contains("too many requests")
        || m.contains("rate_limit")
        || m.contains("ratelimit")
        || m.contains("overloaded")
        || m.contains("throttl")
        || m.contains("tpm")
        || m.contains("rpm")
    {
        return RateLimitKind::Retriable;
    }

    RateLimitKind::No
}

/// Drive the rig multi-turn stream: map each `MultiTurnStreamItem` to a frontend
/// event. Tool calls/results are NOT taken from rig's stream — each tool emits
/// its own richer `tool_call`/`tool_result` from inside `call()`.
///
/// Generic over `R` (the provider's streaming-response type) so OpenAI
/// completions, OpenAI responses, and Anthropic streams all share this body.
async fn run_stream_loop<R>(
    window: tauri::Window,
    task_id: String,
    state: &AppState,
    mut stream: impl futures_util::Stream<Item = Result<MultiTurnStreamItem<R>, StreamingError>> + Unpin,
    input_tokens_est: u64,
    api_format: &str,
    preamble_raw: u64,
    tools_raw: u64,
) -> RunOutcome {
    use futures_util::StreamExt;
    let run_start = std::time::Instant::now();
    let mut run_output_tokens: u64 = 0;
    let mut first_final = true;
    let mut output_buf = String::new();
    let mut emitted_any = false;

    // Check the abort flag before processing each chunk.
    {
        let aborted = state.aborted_tasks.lock().await;
        if aborted.contains(&task_id) {
            drop(aborted);
            state.aborted_tasks.lock().await.remove(&task_id);
            emit_event(&window, &task_id, "done", None, None);
            return RunOutcome::Done;
        }
    }
    while let Some(chunk) = stream.next().await {
        // Check abort mid-stream too.
        {
            let aborted = state.aborted_tasks.lock().await;
            if aborted.contains(&task_id) {
                drop(aborted);
                state.aborted_tasks.lock().await.remove(&task_id);
                emit_event(&window, &task_id, "done", None, None);
                return RunOutcome::Done;
            }
        }
        match chunk {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text_struct))) => {
                emitted_any = true;
                output_buf.push_str(&text_struct.text);
                emit_delta(&window, &task_id, "text", &text_struct.text);
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ReasoningDelta { reasoning, .. })) => {
                emitted_any = true;
                output_buf.push_str(&reasoning);
                emit_delta(&window, &task_id, "reasoning", &reasoning);
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(reasoning_struct))) => {
                emitted_any = true;
                let t = reasoning_struct.display_text();
                output_buf.push_str(&t);
                emit_delta(&window, &task_id, "reasoning", &t);
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            Ok(MultiTurnStreamItem::FinalResponse(final_resp)) => {
                emitted_any = true;
                let rig_usage = final_resp.usage();
                let n = usage::normalize(
                    rig_usage.input_tokens,
                    rig_usage.output_tokens,
                    rig_usage.cached_input_tokens,
                    rig_usage.cache_creation_input_tokens,
                    api_format,
                );
                run_output_tokens += n.completion_tokens;
                let k_sample = if first_final && input_tokens_est > 0 {
                    first_final = false;
                    Some(n.prompt_tokens as f64 / input_tokens_est as f64)
                } else {
                    None
                };
                emit_usage_real(&window, &task_id, n, k_sample, run_output_tokens, preamble_raw, tools_raw);
                output_buf.clear();
            }
            // Tool calls arrive here too, but the tools emit their own events.
            Ok(_) => { emitted_any = true; }
            Err(e) => {
                let msg = e.to_string();
                if !emitted_any && classify_rate_limit_error(&msg) == RateLimitKind::Retriable {
                    return RunOutcome::RateLimited(msg.clone());
                }
                emit_event(&window, &task_id, "error", Some(msg.clone()), None);
                return RunOutcome::Done;
            }
        }
    }

    emit_usage_run_summary(
        &window,
        &task_id,
        run_output_tokens,
        run_start.elapsed().as_millis() as u64,
    );
    RunOutcome::Done
}

/// Current local date+time as `YYYY-MM-DD HH:MM 星期X` (no external dep;
/// std-only epoch math). UTC+8 (CST). Called by the `get_current_time` tool AND
/// by the preamble injection so both paths share one source of truth.
pub(crate) fn current_datetime_str() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let tz_offset_secs: i64 = 8 * 3600;
    let local_secs = secs + tz_offset_secs;
    let days = local_secs.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let hh = (local_secs.rem_euclid(86400)) / 3600;
    let mm = (local_secs.rem_euclid(3600)) / 60;
    let wd = ((days + 4).rem_euclid(7)) as usize;
    let weekday = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][wd];
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02} {weekday}")
}

/// Current local date as `YYYY-MM-DD`.
fn current_date_str() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let tz_offset_secs: i64 = 8 * 3600;
    let local_secs = secs + tz_offset_secs;
    let days = local_secs.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Chinese weekday name for today.
fn weekday_cn() -> &'static str {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let tz_offset_secs: i64 = 8 * 3600;
    let local_secs = secs + tz_offset_secs;
    let days = local_secs.div_euclid(86400);
    let wd = ((days + 4).rem_euclid(7)) as usize;
    ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][wd]
}

/// Inverse of `days_from_civil` (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

pub(crate) async fn run_agent_task_stream(
    window: tauri::Window,
    task_id: String,
    model_id: String,
    provider_id: Option<String>,
    prompt: String,
    history_json: String,
    priority: String,
    confirm_mode: String,
    scenario: Scenario,
    app_state: AppState,
) -> Result<(), String> {
    // 0. 根据当前 task 所属工作区更新 workspace_dir / workspace_path，
    // 确保 OKF 工具读到正确工作区的知识库。
    {
        let conn = crate::db::get_db_conn().map_err(|e| format!("打开 DB 失败: {e}"))?;
        let ws_path: Option<String> = conn
            .prepare("SELECT workspace_path FROM tasks WHERE id = ?")
            .ok()
            .and_then(|mut s| s.query_row([&task_id], |r| r.get::<_, String>(0)).ok());
        if let Some(ws) = ws_path {
            let mut wp = app_state.workspace_path.lock().await;
            *wp = ws.clone();
            drop(wp);
            let mut wd = app_state.workspace_dir.lock().await;
            let aioa_dir = crate::db::get_aioa_dir().unwrap_or_default();
            *wd = aioa_dir.join(&ws);
        }
    }

    // 1. Get model provider config
    let provider = get_provider_for_model(&model_id, provider_id.as_deref())?;

    let effort = match priority.as_str() {
        "最高" => "high",
        "最快" => "low",
        _ => "medium",
    };

    // Anthropic 扩展思考的 budget_tokens（须 >= 1024 且 < max_tokens）。
    // low 不开启思考以优先速度。按格式而非模型名决定是否传思考参数，
    // 避免对每个供应商做特殊处理。
    let thinking_budget = match effort {
        "high" => 16000,
        "medium" => 8000,
        _ => 0,
    };

    let max_tokens_limit = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.max_tokens)
        .unwrap_or(4096) as u64;

    // 2. Parse chat history
    let history: Vec<ChatMessageDto> = serde_json::from_str(&history_json)
        .map_err(|e| format!("解析聊天历史失败: {e}"))?;

    let mut rig_history: Vec<Message> = Vec::new();
    for msg in history {
        let text = get_message_text(&msg);
        if !text.is_empty() {
            if msg.role == "user" {
                rig_history.push(Message::user(text));
            } else if msg.role == "assistant" {
                rig_history.push(Message::assistant(text));
            }
        }
    }

    // Factory closures that build fresh tool instances per scenario. Called
    // once per provider branch AND once per 429-retry (rig consumes the tools
    // when building the agent, so each retry needs a fresh set).
    //
    // rig 的 Tool trait 非 dyn-compatible，无法 Box<dyn Tool> 收集，因此不同
    // scenario 返回不同具体类型元组，在 provider 分支里编译期链式 .tool()。
    let build_general_tools = || -> (GetCurrentTimeTool, SearchTool) {
        (
            GetCurrentTimeTool {
                app_state: app_state.clone(),
                task_id: task_id.clone(),
                window: window.clone(),
            },
            SearchTool {
                app_state: app_state.clone(),
                task_id: task_id.clone(),
                window: window.clone(),
            },
        )
    };
    let build_data_tools = || -> (GetCurrentTimeTool, ExecuteQueryTool, ListTablesTool, DescribeTableTool, SampleDataTool, RenderChartTool, LoadOkfBlockTool, WriteOkfBlockTool, SearchOkfRecipesTool, CreateViewTool, DropObjectTool, ListConnectionsTool, ListRemoteTablesTool, RegisterTableTool) {
        (
            GetCurrentTimeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            ExecuteQueryTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            ListTablesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            DescribeTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            SampleDataTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            RenderChartTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            LoadOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            WriteOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            SearchOkfRecipesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            CreateViewTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), confirm_mode: confirm_mode.clone() },
            DropObjectTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), confirm_mode: confirm_mode.clone() },
            ListConnectionsTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            ListRemoteTablesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            RegisterTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
        )
    };

    // Estimate the input token cost before the stream starts so the UI panel
    // shows data immediately.
    let tool_defs: Vec<_> = match scenario {
        Scenario::General => {
            let (t, s) = build_general_tools();
            vec![t.definition(String::new()).await, s.definition(String::new()).await]
        }
        Scenario::DataAnalysis => {
            let (t, e, l, d, sd, rc, ol, ow, os, cv, doi, lc, lrt, rt) = build_data_tools();
            vec![
                t.definition(String::new()).await,
                e.definition(String::new()).await,
                l.definition(String::new()).await,
                d.definition(String::new()).await,
                sd.definition(String::new()).await,
                rc.definition(String::new()).await,
                ol.definition(String::new()).await,
                ow.definition(String::new()).await,
                os.definition(String::new()).await,
                cv.definition(String::new()).await,
                doi.definition(String::new()).await,
                lc.definition(String::new()).await,
                lrt.definition(String::new()).await,
                rt.definition(String::new()).await,
            ]
        }
    };
    let tools_json = serde_json::to_string(&tool_defs).unwrap_or_default();

    // Inject the current date/time so the agent can resolve relative time
    // expressions like "本月" / "今天" / "下周一" without guessing.
    let now_line = format!(
        "# 当前时间\n现在是 {}（{}）。用户提到「今天」「本周」「下周一」「本月」等相对时间时，以此为准。",
        current_date_str(),
        weekday_cn()
    );

    let base_preamble = match scenario {
        Scenario::General => PREAMBLE,
        Scenario::DataAnalysis => DATA_ANALYSIS_PREAMBLE,
    };

    // 数据分析场景注入工作区 OKF memory summary。
    let memory_summary = match scenario {
        Scenario::DataAnalysis => {
            let ws_dir_str = app_state.workspace_dir.lock().await.to_string_lossy().to_string();
            crate::okf::get_okf_memory_summary(&ws_dir_str)
        }
        Scenario::General => String::new(),
    };

    let combined_preamble = if memory_summary.is_empty() {
        format!("{}\n\n{}", base_preamble, now_line)
    } else {
        format!("{}\n\n{}\n\n# 工作区数据记忆\n以下是你之前探索过的表和知识，直接继承使用，无需重复探索：\n\n{}", base_preamble, now_line, memory_summary)
    };
    let preamble_raw = usage::estimate_tokens(&combined_preamble);
    let tools_raw = usage::estimate_tokens(&tools_json);
    let prompt_t = usage::estimate_tokens(&prompt);
    let history_t: u64 = rig_history
        .iter()
        .map(|m| usage::estimate_tokens(&format!("{:?}", m)))
        .sum();
    let input_est = preamble_raw + tools_raw + prompt_t + history_t;
    emit_usage_estimate(&window, &task_id, input_est, 0, preamble_raw, tools_raw);

    // Retry loop for rate-limit (429) errors. Up to MAX_RETRIES attempts with
    // exponential backoff. Only retries when the 429 arrives before any content
    // was streamed.
    const MAX_RETRIES: usize = 4;
    const BASE_DELAY_SECS: u64 = 5;
    let format = provider.api_format.to_lowercase();
    let mut attempt: usize = 0;
    loop {
        attempt += 1;

        let outcome = match scenario {
            Scenario::General => {
                let (time_tool, search_tool) = build_general_tools();
                if format == "openai" {
                    let base_url = sanitize_endpoint(&provider.endpoint);
                    let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
                        .api_key(&provider.api_key)
                        .base_url(&base_url)
                        .build()
                        .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
                    let mut agent_builder = client
                        .completions_api()
                        .agent(&model_id)
                        .preamble(&combined_preamble)
                        .max_tokens(max_tokens_limit)
                        .tool(time_tool)
                        .tool(search_tool);
                    agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(prompt.clone(), rig_history.clone())
                        .multi_turn(100)
                        .await;
                    run_stream_loop(window.clone(), task_id.clone(), &app_state, stream, input_est, &provider.api_format, preamble_raw, tools_raw).await
                } else if format == "responses" {
                    let base_url = sanitize_endpoint(&provider.endpoint);
                    let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
                        .api_key(&provider.api_key)
                        .base_url(&base_url)
                        .build()
                        .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
                    let mut agent_builder = client
                        .agent(&model_id)
                        .preamble(&combined_preamble)
                        .max_tokens(max_tokens_limit)
                        .tool(time_tool)
                        .tool(search_tool);
                    agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(prompt.clone(), rig_history.clone())
                        .multi_turn(100)
                        .await;
                    run_stream_loop(window.clone(), task_id.clone(), &app_state, stream, input_est, &provider.api_format, preamble_raw, tools_raw).await
                } else if format == "anthropic" {
                    let base_url = sanitize_endpoint(&provider.endpoint);
                    let client: rig_core::providers::anthropic::Client =
                        rig_core::providers::anthropic::Client::builder()
                            .api_key(provider.api_key.clone())
                            .base_url(&base_url)
                            .build()
                            .map_err(|e| format!("构建 Anthropic 客户端失败: {e}"))?;
                    let mut agent_builder = client
                        .agent(&model_id)
                        .preamble(&combined_preamble)
                        .max_tokens(if thinking_budget > 0 { thinking_budget + 4096 } else { 4096 })
                        .tool(time_tool)
                        .tool(search_tool);
                    if thinking_budget > 0 {
                        agent_builder = agent_builder.additional_params(json!({"thinking": {"type": "enabled", "budget_tokens": thinking_budget}}));
                    }
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(prompt.clone(), rig_history.clone())
                        .multi_turn(100)
                        .await;
                    run_stream_loop(window.clone(), task_id.clone(), &app_state, stream, input_est, &provider.api_format, preamble_raw, tools_raw).await
                } else {
                    return Err(format!("不支持的 API 格式: {}", provider.api_format));
                }
            }
            Scenario::DataAnalysis => {
                let (time_tool, exec_tool, list_tool, desc_tool, sample_tool, chart_tool, okf_load_tool, okf_write_tool, okf_search_tool, cv_tool, drop_tool, lc_tool, lrt_tool, rt_tool) = build_data_tools();
                if format == "openai" {
                    let base_url = sanitize_endpoint(&provider.endpoint);
                    let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
                        .api_key(&provider.api_key)
                        .base_url(&base_url)
                        .build()
                        .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
                    let mut agent_builder = client
                        .completions_api()
                        .agent(&model_id)
                        .preamble(&combined_preamble)
                        .max_tokens(max_tokens_limit)
                        .tool(time_tool)
                        .tool(exec_tool)
                        .tool(list_tool)
                        .tool(desc_tool)
                        .tool(sample_tool)
                        .tool(chart_tool)
                        .tool(okf_load_tool)
                        .tool(okf_write_tool)
                        .tool(okf_search_tool)
                        .tool(cv_tool)
                        .tool(drop_tool)
                        .tool(lc_tool)
                        .tool(lrt_tool)
                        .tool(rt_tool);
                    agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(prompt.clone(), rig_history.clone())
                        .multi_turn(100)
                        .await;
                    run_stream_loop(window.clone(), task_id.clone(), &app_state, stream, input_est, &provider.api_format, preamble_raw, tools_raw).await
                } else if format == "responses" {
                    let base_url = sanitize_endpoint(&provider.endpoint);
                    let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
                        .api_key(&provider.api_key)
                        .base_url(&base_url)
                        .build()
                        .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
                    let mut agent_builder = client
                        .agent(&model_id)
                        .preamble(&combined_preamble)
                        .max_tokens(max_tokens_limit)
                        .tool(time_tool)
                        .tool(exec_tool)
                        .tool(list_tool)
                        .tool(desc_tool)
                        .tool(sample_tool)
                        .tool(chart_tool)
                        .tool(okf_load_tool)
                        .tool(okf_write_tool)
                        .tool(okf_search_tool)
                        .tool(cv_tool)
                        .tool(drop_tool)
                        .tool(lc_tool)
                        .tool(lrt_tool)
                        .tool(rt_tool);
                    agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(prompt.clone(), rig_history.clone())
                        .multi_turn(100)
                        .await;
                    run_stream_loop(window.clone(), task_id.clone(), &app_state, stream, input_est, &provider.api_format, preamble_raw, tools_raw).await
                } else if format == "anthropic" {
                    let base_url = sanitize_endpoint(&provider.endpoint);
                    let client: rig_core::providers::anthropic::Client =
                        rig_core::providers::anthropic::Client::builder()
                            .api_key(provider.api_key.clone())
                            .base_url(&base_url)
                            .build()
                            .map_err(|e| format!("构建 Anthropic 客户端失败: {e}"))?;
                    let mut agent_builder = client
                        .agent(&model_id)
                        .preamble(&combined_preamble)
                        .max_tokens(if thinking_budget > 0 { thinking_budget + 4096 } else { 4096 })
                        .tool(time_tool)
                        .tool(exec_tool)
                        .tool(list_tool)
                        .tool(desc_tool)
                        .tool(sample_tool)
                        .tool(chart_tool)
                        .tool(okf_load_tool)
                        .tool(okf_write_tool)
                        .tool(okf_search_tool)
                        .tool(cv_tool)
                        .tool(drop_tool)
                        .tool(lc_tool)
                        .tool(lrt_tool)
                        .tool(rt_tool);
                    if thinking_budget > 0 {
                        agent_builder = agent_builder.additional_params(json!({"thinking": {"type": "enabled", "budget_tokens": thinking_budget}}));
                    }
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(prompt.clone(), rig_history.clone())
                        .multi_turn(100)
                        .await;
                    run_stream_loop(window.clone(), task_id.clone(), &app_state, stream, input_est, &provider.api_format, preamble_raw, tools_raw).await
                } else {
                    return Err(format!("不支持的 API 格式: {}", provider.api_format));
                }
            }
        };

        match outcome {
            RunOutcome::Done => break,
            RunOutcome::RateLimited(_) if attempt <= MAX_RETRIES => {
                let delay = BASE_DELAY_SECS * (1 << (attempt - 1));
                emit_event(
                    &window,
                    &task_id,
                    "text",
                    Some(format!(
                        "（遇到速率限制，{} 秒后自动重试…第 {}/{} 次）",
                        delay, attempt, MAX_RETRIES
                    )),
                    None,
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                continue;
            }
            RunOutcome::RateLimited(last) => {
                emit_event(
                    &window,
                    &task_id,
                    "error",
                    Some(format!(
                        "已自动重试 {MAX_RETRIES} 次仍被速率限制（429），请稍候降低请求频率或更换模型后重试。\n{last}"
                    )),
                    None,
                );
                break;
            }
        }
    }

    emit_event(&window, &task_id, "done", None, None);
    Ok(())
}

#[cfg(test)]
mod tests_rate_limit_classify {
    use super::*;

    #[test]
    fn transient_429_is_retriable() {
        let msg = "Invalid status code 429 Too Many Requests with message: rate_limit_exceeded";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Retriable);
    }

    #[test]
    fn insufficient_quota_is_unretriable() {
        let msg = "Invalid status code 429 with message: insufficient_quota";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Unretriable);
    }

    #[test]
    fn chinese_quota_exhausted_is_unretriable() {
        let msg = "Invalid status code 429 with message: 额度已用尽，请充值";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Unretriable);
    }

    #[test]
    fn auth_error_is_not_rate_limit() {
        let msg = "Invalid status code 401 Unauthorized with message: Invalid API key";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::No);
    }
}
