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
    emit_delta, emit_event, emit_retry_notice, emit_usage_estimate, emit_usage_real,
    emit_usage_run_summary,
};
use super::wire::{ChatMessageDto, Segment};
use crate::skill::builtin::GetCurrentTimeTool;
use crate::skill::data_analysis::{create_view::CreateViewTool, describe_table::DescribeTableTool, delete_okf_knowledge::DeleteOkfKnowledgeTool, drop_object::DropObjectTool, execute_query::ExecuteQueryTool, list_connections::ListConnectionsTool, list_okf_knowledge::ListOkfKnowledgeTool, list_remote_tables::ListRemoteTablesTool, list_tables::ListTablesTool, load_okf_knowledge::LoadOkfKnowledgeTool, read_okf_metadata::ReadOkfMetadataTool, register_table::RegisterTableTool, rename_okf_knowledge::RenameOkfKnowledgeTool, render_chart::RenderChartTool, sample_data::SampleDataTool, search_okf_knowledge::SearchOkfKnowledgeTool, update_okf_metadata::UpdateOkfMetadataTool, write_okf_knowledge::WriteOkfKnowledgeTool};
use crate::skill::search::SearchTool;
use crate::skill::Scenario;
use crate::state::AppState;
use crate::usage::{self};

/// Rebuild the LLM chat history from persisted messages.
///
/// Legacy messages carry a flat `content` string; new messages carry `segments`.
/// Only visible text reaches the model — reasoning and tool steps are managed
/// by rig within the turn and are not replayed as history.
/// Rewrite `{{chart:<token>}}` markers in conclusion text to a readable
/// `[图表:<title>]` before the text is replayed into the LLM history.
///
/// The marker's token is a `call_id` that only resolves against the *current*
/// message's segments. If the literal marker is written back into history, the
/// next turn's model tends to echo the stale token instead of calling
/// `render_chart` again, so the frontend can't find a matching chart in the
/// *new* message and shows "图表引用未找到". Rewriting to the title removes the
/// id from the model's view, preventing cross-turn references at the source.
fn rewrite_chart_markers(
    text: &str,
    chart_titles: &std::collections::HashMap<&str, Option<&str>>,
) -> String {
    const PREFIX: &str = "{{chart:";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(PREFIX) {
        out.push_str(&rest[..start]);
        let after = &rest[start + PREFIX.len()..];
        match after.find("}}") {
            Some(end) => {
                let token = after[..end].trim();
                out.push_str("[图表");
                if let Some(title) = chart_titles.get(token).and_then(|t| *t) {
                    out.push(':');
                    out.push_str(title);
                }
                out.push(']');
                rest = &after[end + 2..];
            }
            None => {
                // Unclosed marker (a streaming mid-state): keep the rest verbatim.
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

fn get_message_text(msg: &ChatMessageDto) -> String {
    if let Some(c) = &msg.content {
        let empty: std::collections::HashMap<&str, Option<&str>> = Default::default();
        return rewrite_chart_markers(c, &empty);
    }
    if let Some(segs) = &msg.segments {
        let chart_titles: std::collections::HashMap<&str, Option<&str>> = segs
            .iter()
            .filter_map(|s| match s {
                Segment::Chart { id, title, .. } => Some((id.as_str(), title.as_deref())),
                _ => None,
            })
            .collect();
        let mut out = String::new();
        for s in segs {
            if let Segment::Text { text, .. } = s {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&rewrite_chart_markers(text, &chart_titles));
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
    /// 恢复性的网络/网关故障（连接被对端中断、网关 5xx/InternalError），且发生
    /// 在任何内容输出之前——重建流重试是安全的。与 RateLimited 共用退避重试。
    Transient(String),
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

/// Detect transient stream/network failures worth retrying when they arrive
/// *before* any content was emitted. 网关上游断连的典型形态：火山网关回
/// `{"code":"InternalError","message":"peer closed connection without sending
/// complete message body (incomplete chunked read)"}`，rig 把这个错误体当流式
/// 事件解析，对外表现为 `Failed to parse JSON: missing field ...`——靠 body
/// 里的标记识别，而不是靠解析错误的表象。另覆盖连接重置与 5xx 状态体。
fn is_transient_stream_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    const MARKERS: &[&str] = &[
        "peer closed connection",
        "incomplete chunked read",
        "connection reset",
        "broken pipe",
        "connection closed before message completed",
        "incompletemessage",
        "internalerror",
        "internal server error",
        "bad gateway",
        "service unavailable",
        "gateway timeout",
        "error decoding response body",
    ];
    if MARKERS.iter().any(|k| m.contains(k)) {
        return true;
    }
    ["status code 500", "status code 502", "status code 503", "status code 504"]
        .iter()
        .any(|k| m.contains(k))
}

/// 把已识别的流错误根因翻译成用户可归因的中文诊断。rig 的 SSE 层把
/// reqwest/hyper 根因展平成了字符串（`ProviderError(String)` 不保留
/// source 链），只能靠标记识别；返回 None 表示无法归类，只展示原始错误。
fn diagnose_stream_error(msg: &str) -> Option<&'static str> {
    let m = msg.to_lowercase();
    if m.contains("peer closed connection")
        || m.contains("incomplete chunked read")
        || m.contains("error decoding response body")
        || m.contains("connection closed before message completed")
        || m.contains("incompletemessage")
    {
        return Some(
            "服务端在流式传输中途断开了连接（响应体不完整）。常见原因：网络波动、\
             代理/VPN 掐断长连接、模型服务或网关过载超时",
        );
    }
    if m.contains("connection reset") || m.contains("broken pipe") {
        return Some("TCP 连接被重置。常见原因：网络切换、防火墙/代理干预、服务端过载");
    }
    if m.contains("internalerror")
        || m.contains("internal server error")
        || m.contains("bad gateway")
        || m.contains("service unavailable")
        || m.contains("gateway timeout")
        || m.contains("status code 50")
    {
        return Some("模型服务端/网关内部错误（5xx 类），多为服务端瞬时故障");
    }
    if m.contains("timed out") || m.contains("timeout") || m.contains("timedout") {
        return Some("请求超时。可能网络延迟过大，或服务端排队过长");
    }
    None
}

/// Detect provider-side rejections of model-emitted tool calls that don't
/// match the registered tool list (e.g. 火山 plan 端点把不认识的工具名整请求
/// 打回: `PromptError: UnknownToolCall: model attempted to call unknown or
/// disallowed tool ...`). ratelimit 分类器处理不了这类错误，单独识别以便给
/// 用户可操作的提示。
fn is_unknown_tool_call_error(msg: &str) -> bool {
    msg.contains("UnknownToolCall")
        || msg.contains("unknown or disallowed tool")
        || msg.contains("未知工具")
        || msg.contains("不存在的工具")
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
    // 本条消息内已通过 ReasoningDelta 发出的思考文本。完整 Reasoning 事件
    // 到达时用它做前缀去重——两者都当增量发会把思考段逐字翻倍（UI 与 jsonl
    // 双写，2026-08-27 复盘实证每段 reasoning 存了两遍）。
    let mut reasoning_delta_buf = String::new();

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
                reasoning_delta_buf.push_str(&reasoning);
                emit_delta(&window, &task_id, "reasoning", &reasoning);
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(reasoning_struct))) => {
                emitted_any = true;
                let t = reasoning_struct.display_text();
                // 增量已发过的部分只补发余量；无增量路径（非流式思考）发全文。
                // 前缀对不上（如服务端终稿与增量不一致）时保守发全文。
                let already = std::mem::take(&mut reasoning_delta_buf);
                let remainder = t.strip_prefix(&already).unwrap_or(&t).to_string();
                output_buf.push_str(&remainder);
                emit_delta(&window, &task_id, "reasoning", &remainder);
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
                reasoning_delta_buf.clear();
            }
            // Tool calls arrive here too, but the tools emit their own events.
            Ok(_) => { emitted_any = true; }
            Err(e) => {
                let mut msg = e.to_string();
                if !emitted_any && classify_rate_limit_error(&msg) == RateLimitKind::Retriable {
                    return RunOutcome::RateLimited(msg.clone());
                }
                if !emitted_any && is_transient_stream_error(&msg) {
                    return RunOutcome::Transient(msg.clone());
                }
                // 未走重试的流错误：附上根因诊断 + 中断位置（阶段/token/耗时），
                // 并落结构化日志（daw.db logs 表，category=agent）便于事后归因。
                let stage_cn = if emitted_any { "流式输出过程中" } else { "请求建立阶段" };
                let approx_out = run_output_tokens + usage::estimate_tokens(&output_buf);
                let elapsed_secs = run_start.elapsed().as_secs();
                tracing::warn!(
                    category = "agent",
                    task_id = task_id.as_str(),
                    detail = serde_json::to_string(&serde_json::json!({
                        "stage": if emitted_any { "mid_stream" } else { "before_output" },
                        "elapsedSecs": elapsed_secs,
                        "outputTokensApprox": approx_out,
                        "diagnosis": diagnose_stream_error(&msg),
                        "raw": msg.clone(),
                    })).unwrap_or_default(),
                    "LLM 流式中断（{stage_cn}，已输出约 {approx_out} tokens，{elapsed_secs}s）"
                );
                if let Some(d) = diagnose_stream_error(&msg) {
                    msg.push_str(&format!(
                        "\n\n【原因】{d}。\n【位置】{stage_cn}（已输出约 {approx_out} tokens，耗时 {elapsed_secs} 秒）{}。",
                        if emitted_any { "；已生成的部分内容已保留" } else { "" }
                    ));
                    msg.push_str("\n可稍后重发继续；频繁出现请检查网络/代理，或更换模型端点。");
                }
                if is_unknown_tool_call_error(&msg) {
                    msg.push_str(
                        "\n\n模型尝试调用了一个不在工具列表中的函数，请求被服务端否决。\
                         这类失败通常由模型臆造工具名引起，直接重试或换个模型即可恢复。",
                    );
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
    // 0. 解析本任务所属工作区，并（数据分析场景）获取该工作区的 DuckDB 连接 +
    //    赢得每工作区单流锁。workspace 指针按任务注入到工具，不再写共享
    //    AppState 字段——这样跨工作区并发任务不会互相覆盖 workspace 指针 /
    //    默认 catalog，每个工作区的视图落进各自的 lake。
    let ws_path = {
        let conn = crate::db::get_db_conn().map_err(|e| format!("打开 DB 失败: {e}"))?;
        conn.prepare("SELECT workspace_path FROM tasks WHERE id = ?")
            .ok()
            .and_then(|mut s| s.query_row([&task_id], |r| r.get::<_, String>(0)).ok())
            .unwrap_or_else(|| "DefaultProject".to_string())
    };
    let ws_dir = crate::db::get_app_dir().unwrap_or_default().join(&ws_path);
    let ws_ref = crate::skill::WorkspaceRef { path: ws_path.clone(), dir: ws_dir.clone() };

    // 数据分析场景：懒创建/复用该工作区的 DuckDB 连接（各工作区 lake 隔离），
    // 并赢得单流锁（同工作区同一时刻只允许一个数据分析任务；跨工作区并发）。
    // `_busy_guard` 在函数返回时 Drop，自动释放单流锁。
    let _busy_guard: Option<crate::state::BusyGuard> = if scenario == Scenario::DataAnalysis {
        let wsc = app_state
            .ensure_workspace_conn(&ws_path)
            .await
            .map_err(|e| format!("数据分析环境未就绪: {e}"))?;
        if wsc.busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err(format!(
                "工作区「{ws_path}」已有数据分析任务在运行，请等待其完成后再试"
            ));
        }
        Some(crate::state::BusyGuard::new(wsc))
    } else {
        None
    };

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
    let build_data_tools = || -> (GetCurrentTimeTool, SearchTool, ExecuteQueryTool, ListTablesTool, DescribeTableTool, SampleDataTool, RenderChartTool, LoadOkfKnowledgeTool, WriteOkfKnowledgeTool, SearchOkfKnowledgeTool, CreateViewTool, DropObjectTool, ListConnectionsTool, ListRemoteTablesTool, RegisterTableTool, ReadOkfMetadataTool, UpdateOkfMetadataTool, ListOkfKnowledgeTool, DeleteOkfKnowledgeTool, RenameOkfKnowledgeTool) {
        (
            GetCurrentTimeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            SearchTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            ExecuteQueryTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            ListTablesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            DescribeTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            SampleDataTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            RenderChartTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            LoadOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            WriteOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            SearchOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            CreateViewTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), confirm_mode: confirm_mode.clone(), ws: ws_ref.clone() },
            DropObjectTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), confirm_mode: confirm_mode.clone(), ws: ws_ref.clone() },
            ListConnectionsTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            ListRemoteTablesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            RegisterTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            ReadOkfMetadataTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            UpdateOkfMetadataTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            ListOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            DeleteOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
            RenameOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), ws: ws_ref.clone() },
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
            let (t, s, e, l, d, sd, rc, ol, ow, os, cv, doi, lc, lrt, rt, rm, um, lk, dk, rk) = build_data_tools();
            vec![
                t.definition(String::new()).await,
                s.definition(String::new()).await,
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
                rm.definition(String::new()).await,
                um.definition(String::new()).await,
                lk.definition(String::new()).await,
                dk.definition(String::new()).await,
                rk.definition(String::new()).await,
            ]
        }
    };
    let tools_json = serde_json::to_string(&tool_defs).unwrap_or_default();

    // Brand name from ~/.daw/brand.json shapes the agent's self-identity in
    // the preamble (defaults to "Daw").
    let app_name = crate::brand::load_brand().app_name;
    // system prompt 只放静态纪律：品牌名注入后跨轮/跨会话字节稳定，provider
    // 的前缀缓存才能持续命中。每轮变化的事实（当前时间、OKF 大纲）走下方
    // runtime_context 快照——此前它们拼在 preamble 里，每写一条知识，下一轮
    // 整个 system prompt 前缀缓存就被击穿。
    let combined_preamble = match scenario {
        Scenario::General => usage::general_preamble(&app_name),
        Scenario::DataAnalysis => usage::data_analysis_preamble(&app_name),
    };

    // Inject the current date/time so the agent can resolve relative time
    // expressions like "本月" / "今天" / "下周一" without guessing.
    let now_line = format!(
        "# 当前时间\n现在是 {}（{}）。用户提到「今天」「本周」「下周一」「本月」等相对时间时，以此为准。",
        current_date_str(),
        weekday_cn()
    );

    // 运行时上下文快照：随本次用户消息下发，不进 system prompt、不写进持久
    // 化历史（前端只存用户原文），下一轮自然被新快照取代。模型看到的布局：
    // <runtime_context> 块 + 用户原文。
    let runtime_context = match scenario {
        Scenario::DataAnalysis => {
            let entries = crate::db::list_table_registry(&ws_path).unwrap_or_default();
            let outline = crate::okf::Okf::production().catalog_summary(&ws_path, &entries);
            if outline.is_empty() {
                now_line
            } else {
                format!("{now_line}\n\n{outline}")
            }
        }
        Scenario::General => now_line,
    };
    let llm_prompt = format!("<runtime_context>\n{runtime_context}\n</runtime_context>\n\n{prompt}");

    let preamble_raw = usage::estimate_tokens(&combined_preamble);
    let tools_raw = usage::estimate_tokens(&tools_json);
    let prompt_t = usage::estimate_tokens(&llm_prompt);
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
                        .stream_chat(llm_prompt.clone(), rig_history.clone())
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
                        .stream_chat(llm_prompt.clone(), rig_history.clone())
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
                        .stream_chat(llm_prompt.clone(), rig_history.clone())
                        .multi_turn(100)
                        .await;
                    run_stream_loop(window.clone(), task_id.clone(), &app_state, stream, input_est, &provider.api_format, preamble_raw, tools_raw).await
                } else {
                    return Err(format!("不支持的 API 格式: {}", provider.api_format));
                }
            }
            Scenario::DataAnalysis => {
                let (time_tool, search_tool, exec_tool, list_tool, desc_tool, sample_tool, chart_tool, okf_load_tool, okf_write_tool, okf_search_tool, cv_tool, drop_tool, lc_tool, lrt_tool, rt_tool, okf_meta_read_tool, okf_meta_update_tool, okf_list_tool, okf_delete_tool, okf_rename_tool) = build_data_tools();
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
                        .tool(search_tool)
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
                        .tool(rt_tool)
                        .tool(okf_meta_read_tool)
                        .tool(okf_meta_update_tool)
                        .tool(okf_list_tool)
                        .tool(okf_delete_tool)
                        .tool(okf_rename_tool);
                    agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(llm_prompt.clone(), rig_history.clone())
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
                        .tool(search_tool)
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
                        .tool(rt_tool)
                        .tool(okf_meta_read_tool)
                        .tool(okf_meta_update_tool)
                        .tool(okf_list_tool)
                        .tool(okf_delete_tool)
                        .tool(okf_rename_tool);
                    agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(llm_prompt.clone(), rig_history.clone())
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
                        .tool(search_tool)
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
                        .tool(rt_tool)
                        .tool(okf_meta_read_tool)
                        .tool(okf_meta_update_tool)
                        .tool(okf_list_tool)
                        .tool(okf_delete_tool)
                        .tool(okf_rename_tool);
                    if thinking_budget > 0 {
                        agent_builder = agent_builder.additional_params(json!({"thinking": {"type": "enabled", "budget_tokens": thinking_budget}}));
                    }
                    let agent = agent_builder.build();
                    let stream = agent
                        .stream_chat(llm_prompt.clone(), rig_history.clone())
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
            RunOutcome::RateLimited(_) | RunOutcome::Transient(_) if attempt <= MAX_RETRIES => {
                let delay = BASE_DELAY_SECS * (1 << (attempt - 1));
                emit_retry_notice(
                    &window,
                    &task_id,
                    attempt as u32,
                    MAX_RETRIES as u32,
                    delay,
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
            RunOutcome::Transient(last) => {
                let cause = diagnose_stream_error(&last)
                    .map(|d| format!("：{d}"))
                    .unwrap_or_default();
                emit_event(
                    &window,
                    &task_id,
                    "error",
                    Some(format!(
                        "网络或服务端连接中断{cause}。已自动重试 {MAX_RETRIES} 次仍失败，请稍后重发或更换模型。\n{last}"
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

    #[test]
    fn volcano_gateway_internalerror_is_transient() {
        // 火山网关上游断连：错误体被 rig 当流式事件解析成 missing field，标记在 body 里。
        let msg = "CompletionError: ResponseError: Failed to parse JSON: missing field `type` at line 1 column 175 (Data: {\"request_id\":\"3497554d-655c-4f19-8941-5bd5471982da\",\"code\":\"InternalError\",\"message\":\"peer closed connection without sending complete message body (incomplete chunked read)\"})";
        assert!(is_transient_stream_error(msg));
    }

    #[test]
    fn connection_drop_markers_are_transient() {
        assert!(is_transient_stream_error(
            "reqwest error: error sending request for url: connection reset by peer"
        ));
        assert!(is_transient_stream_error("hyper::Error(IncompleteMessage)"));
        assert!(is_transient_stream_error(
            "Invalid status code 503 Service Unavailable with message: upstream busy"
        ));
        assert!(is_transient_stream_error(
            "Invalid status code 502 Bad Gateway with message: bad gateway"
        ));
    }

    #[test]
    fn quota_and_auth_errors_are_not_transient() {
        assert!(!is_transient_stream_error(
            "Invalid status code 429 with message: insufficient_quota"
        ));
        assert!(!is_transient_stream_error(
            "Invalid status code 401 Unauthorized with message: Invalid API key"
        ));
        // 无瞬态标记的纯解析错误不重试（可能是 schema 变更，重试无意义）。
        assert!(!is_transient_stream_error(
            "Failed to parse JSON: missing field `type` at line 1 column 175"
        ));
    }

    #[test]
    fn diagnose_translates_known_root_causes() {
        // 用户实际遇到的两种形态：网关 InternalError 断连、流式 body 中断。
        let d = diagnose_stream_error(
            "CompletionError: ProviderError: SSE Error: Http client error: error decoding response body",
        );
        assert!(d.unwrap().contains("流式传输中途断开"));
        let d = diagnose_stream_error(
            "CompletionError: ResponseError: Failed to parse JSON: missing field `type` (Data: {\"code\":\"InternalError\",\"message\":\"peer closed connection without sending complete message body (incomplete chunked read)\"})",
        );
        assert!(d.unwrap().contains("流式传输中途断开"));
        let d = diagnose_stream_error("reqwest error: connection reset by peer");
        assert!(d.unwrap().contains("TCP 连接被重置"));
        let d = diagnose_stream_error("Invalid status code 502 Bad Gateway with message: x");
        assert!(d.unwrap().contains("5xx"));
        let d = diagnose_stream_error("operation timed out");
        assert!(d.unwrap().contains("超时"));
        // 无法归类/不该归类的返回 None：配额、认证、纯解析错误。
        assert_eq!(diagnose_stream_error("Invalid status code 429 with message: insufficient_quota"), None);
        assert_eq!(diagnose_stream_error("Invalid status code 401 Unauthorized"), None);
        assert_eq!(diagnose_stream_error("Failed to parse JSON: missing field `type`"), None);
    }
}
