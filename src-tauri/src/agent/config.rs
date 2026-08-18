//! Settings.json reading, model/provider lookup, and endpoint sanitization.
//!
//! (Ported from the earlier data-lake prototype: the settings path moved under
//! `~/.daw`; the `query_timeout`/`query_hard_timeout` fields were removed —
//! those gated DuckDB SQL execution and have no equivalent here. Provider/model
//! lookup and endpoint sanitization are unchanged.)

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSettings {
    providers: Vec<ModelProvider>,
}

/// Read the settings.json file once and return the parsed struct.
///
/// M1-unused: only consumed by [`first_enabled_model`], which is itself an M2
/// entry point (background LLM tasks). Kept so the call chain compiles; remove
/// this allow when M2 wires it up.
#[allow(dead_code)]
fn read_saved_settings() -> SavedSettings {
    let fallback = SavedSettings {
        providers: Vec::new(),
    };
    let mut path = match crate::db::get_app_dir() {
        Ok(p) => p,
        Err(_) => return fallback,
    };
    path.push("settings.json");
    if !path.exists() {
        return fallback;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return fallback,
    };
    serde_json::from_str(&content).unwrap_or(fallback)
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelItem {
    pub(crate) id: String,
    #[allow(dead_code)]
    pub(crate) context_window: usize,
    pub(crate) max_tokens: Option<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelProvider {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) api_key: String,
    pub(crate) api_format: String, // "openai" | "anthropic" | "responses"
    pub(crate) models: Vec<ModelItem>,
    pub(crate) enabled: bool,
}

/// Resolve the provider+model config for a chat/completion request.
///
/// When `provider_id` is `Some`, the provider with that exact id is preferred
/// (this disambiguates duplicate model ids across providers). If the given
/// provider id isn't found or doesn't carry `model_id`, we fall back to the
/// first enabled provider containing the model — preserving backward
/// compatibility with older tasks/defaults persisted as a bare model id.
pub(crate) fn get_provider_for_model(
    model_id: &str,
    provider_id: Option<&str>,
) -> Result<ModelProvider, String> {
    let mut path = crate::db::get_app_dir()?;
    path.push("settings.json");
    if !path.exists() {
        return Err("配置文件 settings.json 不存在，请先在设置中配置模型。".to_string());
    }
    let content = std::fs::read_to_string(path).map_err(|e| format!("读取配置文件失败: {e}"))?;
    let settings: SavedSettings =
        serde_json::from_str(&content).map_err(|e| format!("解析配置文件失败: {e}"))?;

    // 1. Prefer the exact provider when one is supplied and matches.
    if let Some(pid) = provider_id {
        for provider in &settings.providers {
            if provider.enabled && provider.id == pid {
                if provider.models.iter().any(|m| m.id == model_id) {
                    return Ok(provider.clone());
                }
            }
        }
    }

    // 2. Fall back to the first enabled provider carrying this model id.
    for provider in settings.providers {
        if provider.enabled {
            if provider.models.iter().any(|m| m.id == model_id) {
                return Ok(provider);
            }
        }
    }

    Err(format!(
        "未找到包含模型「{}」且已启用的服务商，请检查设置。",
        model_id
    ))
}

/// Return the id of the first enabled model in settings.json, or `None` when no
/// provider is configured.
///
/// M2 entry point: background LLM tasks (e.g. auto-generating a leave-request
/// title, conversation summaries) need a default model pick. Unused in M1.
#[allow(dead_code)]
pub(crate) fn first_enabled_model() -> Option<String> {
    let settings = read_saved_settings();
    for provider in settings.providers {
        if provider.enabled {
            if let Some(m) = provider.models.first() {
                return Some(m.id.clone());
            }
        }
    }
    None
}

/// Strip common API-path suffixes from a user-supplied endpoint so it becomes a
/// valid base URL for the rig provider clients (which append their own paths).
pub(crate) fn sanitize_endpoint(endpoint: &str) -> String {
    let mut clean = endpoint.trim().to_string();
    while clean.ends_with('/') {
        clean.pop();
    }
    if clean.ends_with("/chat/completions") {
        clean = clean[..clean.len() - "/chat/completions".len()].to_string();
    } else if clean.ends_with("/v1/chat/completions") {
        clean = clean[..clean.len() - "/v1/chat/completions".len()].to_string();
    } else if clean.ends_with("/v1/messages") {
        clean = clean[..clean.len() - "/v1/messages".len()].to_string();
    } else if clean.ends_with("/messages") {
        clean = clean[..clean.len() - "/messages".len()].to_string();
    }
    while clean.ends_with('/') {
        clean.pop();
    }
    clean
}
