//! Application-wide singleton state, owned by `tauri::State`.
//!
//! Domain-agnostic machinery: the human-confirmation channel for write
//! operations, the abort flag for stopping a running stream, and the
//! current workspace path. No domain-specific state here — skills own
//! their data dependencies.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::sync::oneshot;

use crate::db::get_home_dir;

/// User's decision on whether a pending write operation should proceed.
#[derive(Debug, Clone)]
pub struct ConfirmDecision {
    pub approved: bool,
}

/// A write tool invocation parked in "变更前确认" mode, waiting for the user to
/// approve or cancel it from the UI. The `oneshot::Sender` is used to resume the
/// blocked tool `call()`.
pub struct PendingConfirmation {
    pub tx: oneshot::Sender<ConfirmDecision>,
}

/// App-wide singleton: skill registry + cross-tool coordination state.
#[derive(Clone)]
pub struct AppState {
    /// Absolute path of the workspace directory currently active.
    ///
    /// M2 entry point: skills that touch workspace-scoped data will read this.
    #[allow(dead_code)]
    pub workspace_dir: Arc<Mutex<PathBuf>>,
    /// The workspace key (`workspaces.path`) currently active, e.g. "DefaultProject".
    #[allow(dead_code)]
    pub workspace_path: Arc<Mutex<String>>,
    /// Write tool calls parked in "变更前确认" mode, keyed by `{task_id}:{tool_call_id}`.
    /// Each entry holds a oneshot sender that resumes the blocked tool once the
    /// user approves or cancels from the UI (via `resolve_tool_confirmation`).
    pub pending_confirmations: Arc<Mutex<HashMap<String, PendingConfirmation>>>,
    /// Aborted task IDs. Inserted by `abort_chat`; checked by `run_stream_loop`
    /// each iteration so a long-running stream stops promptly.
    pub aborted_tasks: Arc<Mutex<HashSet<String>>>,
    /// The currently active space id ("personal" or an enterprise UUID). The
    /// source of truth is `settings.json`'s `activeSpace`; this is an in-memory
    /// cache seeded at startup and kept in sync by `set_active_space` /
    /// `leave_enterprise`.
    pub active_space: Arc<Mutex<String>>,
    /// 搜索后端（None=未配置搜索服务，SearchTool 调时报错提示）。
    pub search_backend: Option<Arc<dyn crate::skill::search::SearchBackend>>,
}

impl AppState {
    pub fn new() -> Self {
        let ws = default_workspace_dir();
        Self {
            workspace_dir: Arc::new(Mutex::new(ws)),
            workspace_path: Arc::new(Mutex::new("DefaultProject".to_string())),
            pending_confirmations: Arc::new(Mutex::new(HashMap::new())),
            aborted_tasks: Arc::new(Mutex::new(HashSet::new())),
            active_space: Arc::new(Mutex::new(load_active_space())),
            search_backend: crate::skill::search::create_search_backend_from_settings(),
        }
    }
}

/// Read the active space from `~/.aioa/settings.json` so the in-memory cache
/// matches what was last persisted. Falls back to "personal" when the file or
/// field is missing/unreadable (the default for a fresh install).
fn load_active_space() -> String {
    let Some(mut home) = get_home_dir() else {
        return "personal".to_string();
    };
    home.push(".aioa");
    home.push("settings.json");
    let Ok(content) = std::fs::read_to_string(&home) else {
        return "personal".to_string();
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) else {
        return "personal".to_string();
    };
    val.get("activeSpace")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "personal".to_string())
}

/// The default workspace directory: `~/.aioa/DefaultProject/`.
fn default_workspace_dir() -> PathBuf {
    let mut home = get_home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.push(".aioa");
    home.push("DefaultProject");
    home
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
