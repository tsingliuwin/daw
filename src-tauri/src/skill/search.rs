//! 搜索工具——基座内置工具，支持多搜索服务切换。
//!
//! 对 LLM 暴露一个 `search` 工具（rig Tool trait），内部用 `SearchBackend` trait
//! 切换搜索服务（Exa/Brave/未来）。后端选择是配置问题，不暴露给 LLM。
//!
//! 配置存 settings.json: `{ "search": { "engine": "exa", "apiKey": "exa-xxx" } }`
//! runner 构造 SearchTool 时从 AppState 拿 search_backend（None=未配置，调时报错）。

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use crate::agent::error::ToolError;
use crate::agent::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// 搜索结果
// ---------------------------------------------------------------------------

/// 一条搜索结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    /// 摘要/highlights。
    pub snippet: String,
}

// ---------------------------------------------------------------------------
// SearchBackend trait——多搜索服务抽象
// ---------------------------------------------------------------------------

/// 搜索后端抽象。每个搜索服务（Exa/Brave/...）实现此 trait。
#[async_trait]
pub trait SearchBackend: Send + Sync {
    /// 执行搜索，返回结果列表。
    async fn search(&self, query: &str, num_results: usize) -> Result<Vec<SearchResult>, String>;
    /// 后端显示名（如 "Exa"）。
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Exa 搜索后端
// ---------------------------------------------------------------------------

/// Exa (exa.ai) 搜索后端。
pub struct ExaSearchBackend {
    api_key: String,
    client: reqwest::Client,
}

impl ExaSearchBackend {
    pub fn new(api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            api_key,
            client,
        }
    }
}

#[async_trait]
impl SearchBackend for ExaSearchBackend {
    fn name(&self) -> &str {
        "Exa"
    }

    async fn search(&self, query: &str, num_results: usize) -> Result<Vec<SearchResult>, String> {
        let body = json!({
            "query": query,
            "type": "auto",
            "numResults": num_results,
            "contents": { "highlights": true }
        });

        // 重试：网络偶发失败时等 1 秒重试一次（Exa 不限重试，QPS 10 够用）。
        let mut last_err = String::new();
        for attempt in 0..2u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let resp = match self
                .client
                .post("https://api.exa.ai/search")
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("Exa 搜索请求失败: {e}");
                    continue; // 重试
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                last_err = format!("Exa 搜索失败 (HTTP {status}): {text}");
                // 429/5xx 重试，4xx 不重试
                if status.as_u16() >= 500 || status.as_u16() == 429 {
                    continue;
                }
                return Err(last_err);
            }

            let data: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("Exa 响应解析失败: {e}"))?;

            let results = data["results"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|item| SearchResult {
                            title: item["title"].as_str().unwrap_or("").to_string(),
                            url: item["url"].as_str().unwrap_or("").to_string(),
                            snippet: {
                                let h = item["highlights"].as_array();
                                h.map(|arr| arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n"))
                                    .unwrap_or_else(|| item["text"].as_str().unwrap_or("").to_string())
                            },
                        })
                        .collect()
                })
                .unwrap_or_default();

            return Ok(results);
        } // end for

        Err(last_err)
    }
}

// ---------------------------------------------------------------------------
// 豆包搜索后端（火山引擎）
// ---------------------------------------------------------------------------

/// 豆包搜索（火山引擎）后端。
///
/// 文档: https://docs.volcengine.com/docs/87772/2272953
/// API Key 接入: POST https://open.feedcoopapi.com/search_api/web_search
/// 鉴权: Authorization: Bearer <API_KEY>
pub struct DoubaoSearchBackend {
    api_key: String,
    client: reqwest::Client,
}

impl DoubaoSearchBackend {
    pub fn new(api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { api_key, client }
    }
}

#[async_trait]
impl SearchBackend for DoubaoSearchBackend {
    fn name(&self) -> &str {
        "豆包"
    }

    async fn search(&self, query: &str, num_results: usize) -> Result<Vec<SearchResult>, String> {
        // 豆包 Custom 版 web 搜索。NeedUrl=true 过滤掉无链接的"火山如意"卡片结果，
        // 保证每条结果都有可点击的落地页。
        let body = json!({
            "Query": query,
            "SearchType": "web",
            "Count": num_results,
            "Filter": { "NeedUrl": true }
        });

        // 重试：网络偶发失败或服务端 10500/700429 时等 1 秒重试一次（默认 5 QPS 够用）。
        let mut last_err = String::new();
        for attempt in 0..2u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let resp = match self
                .client
                .post("https://open.feedcoopapi.com/search_api/web_search")
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("豆包搜索请求失败: {e}");
                    continue; // 重试
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                last_err = format!("豆包搜索失败 (HTTP {status}): {text}");
                // 429/5xx 重试，4xx 不重试
                if status.as_u16() >= 500 || status.as_u16() == 429 {
                    continue;
                }
                return Err(last_err);
            }

            let data: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("豆包响应解析失败: {e}"))?;

            // 豆包即使在 HTTP 200 下也可能返回业务错误（ResponseMetadata.Error）。
            // 10500 InnerError / 700429 限流 可重试；其它业务错误（参数/权限/额度）不重试。
            if let Some(err) = data.get("ResponseMetadata").and_then(|m| m.get("Error")) {
                let code = err
                    .get("CodeN")
                    .and_then(|v| v.as_i64())
                    .or_else(|| {
                        err.get("Code")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<i64>().ok())
                    })
                    .unwrap_or(0);
                let message = err
                    .get("Message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知错误");
                last_err = format!("豆包搜索失败 ({code}): {message}");
                if code == 10500 || code == 700429 {
                    continue;
                }
                return Err(last_err);
            }

            let results = data
                .get("Result")
                .and_then(|r| r.get("WebResults"))
                .and_then(|w| w.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|item| SearchResult {
                            title: item["Title"].as_str().unwrap_or("").to_string(),
                            url: item["Url"].as_str().unwrap_or("").to_string(),
                            // 文档推荐 Summary 用于大模型场景（500~1000字），
                            // Snippet 仅约200字且"强烈不建议用于大模型"，作为 fallback。
                            snippet: item["Summary"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .or_else(|| item["Snippet"].as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            return Ok(results);
        } // end for

        Err(last_err)
    }
}

// ---------------------------------------------------------------------------
// SearchTool--对 LLM 暴露的 rig Tool
// ---------------------------------------------------------------------------

/// 搜索工具的参数。
#[derive(Deserialize, Serialize)]
pub struct SearchArgs {
    /// 搜索关键词。
    query: String,
    /// 结果数量（默认 5，上限 10）。
    #[serde(default)]
    num_results: Option<usize>,
}

pub struct SearchTool {
    #[allow(dead_code)]
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
}

impl Tool for SearchTool {
    const NAME: &'static str = "search";
    type Error = ToolError;
    type Args = SearchArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "search".to_string(),
            description: "搜索互联网获取最新信息。当用户问需要实时信息、外部知识、最新新闻、或你不确定的事实性问题时，调用本工具。返回网页搜索结果（标题、链接、摘要）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "搜索关键词"
                    },
                    "num_results": {
                        "type": "number",
                        "description": "结果数量，默认5，最大10"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("search");
        emit_tool_call(
            &self.window,
            &self.task_id,
            &call_id,
            "search",
            json!({ "query": args.query, "num_results": args.num_results }),
        );
        let start = std::time::Instant::now();

        let num = args.num_results.unwrap_or(5).min(10).max(1);

        // 每次调用时实时从 settings.json 读取搜索配置（用户可能在应用启动后
        // 才在设置页配的，启动时缓存的 search_backend 可能是 None）。
        let backend = create_search_backend_from_settings();
        let result = match &backend {
            Some(b) => b.search(&args.query, num).await,
            None => Err("未配置搜索服务。请在设置 → 通用 → 搜索服务中配置 API Key。".to_string()),
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(results) => {
                // 格式化结果给 LLM。
                let formatted = results
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        format!(
                            "{}. [{}]({})\n   {}",
                            i + 1,
                            r.title,
                            r.url,
                            r.snippet.lines().take(3).collect::<Vec<_>>().join("\n   ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let summary = format!("找到 {} 条结果:\n\n{}", results.len(), formatted);
                let payload = json!({
                    "engine": backend.as_ref().map(|b| b.name()).unwrap_or(""),
                    "query": args.query,
                    "count": results.len(),
                    "results": results,
                });
                emit_tool_result(
                    &self.window,
                    &self.task_id,
                    &call_id,
                    "ok",
                    summary.clone(),
                    None,
                    Some(payload),
                    Some(elapsed),
                    None,
                );
                Ok(summary)
            }
            Err(msg) => {
                emit_tool_result(
                    &self.window,
                    &self.task_id,
                    &call_id,
                    "error",
                    msg.clone(),
                    None,
                    None,
                    Some(elapsed),
                    None,
                );
                Err(ToolError(msg))
            }
        }
    }
}

/// 从 settings.json 读搜索配置，构造对应的 SearchBackend。
/// 返回 None 表示未配置搜索服务。
pub fn create_search_backend_from_settings() -> Option<Arc<dyn SearchBackend>> {
    let mut path = crate::db::get_aioa_dir().ok()?;
    path.push("settings.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let settings: serde_json::Value = serde_json::from_str(&content).ok()?;
    let search = settings.get("search")?;
    let engine = search.get("engine")?.as_str()?;
    let api_key = search.get("apiKey")?.as_str()?.to_string();
    if api_key.is_empty() {
        return None;
    }
    match engine {
        "exa" => Some(Arc::new(ExaSearchBackend::new(api_key))),
        "doubao" => Some(Arc::new(DoubaoSearchBackend::new(api_key))),
        _ => None,
    }
}
