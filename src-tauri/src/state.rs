//! Application-wide singleton state, owned by `tauri::State`.
//!
//! Domain-agnostic machinery: the human-confirmation channel for write
//! operations, the abort flag for stopping a running stream, and the
//! current workspace path. No domain-specific state here - skills own
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
    /// 搜索后端（None=未配置搜索服务，SearchTool 调时报错提示）。
    pub search_backend: Option<Arc<dyn crate::skill::search::SearchBackend>>,
    /// DuckDB in-memory 会话。数据分析场景的查询引擎。
    /// None 表示 DuckDB 打开失败（应用仍可用，但数据分析工具调时会报错）。
    pub duckdb: Option<Arc<Mutex<duckdb::Connection>>>,
    /// DuckDB 中断句柄（单独存放，超时线程 interrupt 时不锁 conn 避免死锁）。
    pub interrupt_handle: Option<Arc<std::sync::Mutex<Arc<duckdb::InterruptHandle>>>>,
}

impl AppState {
    pub fn new() -> Self {
        let ws = default_workspace_dir();
        let (duckdb_conn, interrupt_handle) = open_duckdb();
        Self {
            workspace_dir: Arc::new(Mutex::new(ws)),
            workspace_path: Arc::new(Mutex::new("DefaultProject".to_string())),
            pending_confirmations: Arc::new(Mutex::new(HashMap::new())),
            aborted_tasks: Arc::new(Mutex::new(HashSet::new())),
            search_backend: crate::skill::search::create_search_backend_from_settings(),
            duckdb: duckdb_conn,
            interrupt_handle,
        }
    }
}

/// 打开 DuckDB in-memory 会话，ATTACH settings.json 中配置的全部数据源。
/// 失败时返回 (None, None)，应用仍可启动（数据分析工具调时报错）。
fn open_duckdb() -> (
    Option<Arc<Mutex<duckdb::Connection>>>,
    Option<Arc<std::sync::Mutex<Arc<duckdb::InterruptHandle>>>>,
) {
    let conn = match duckdb::Connection::open_in_memory() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(category = "system", "DuckDB 打开失败: {e}");
            return (None, None);
        }
    };
    // 限制内存使用（不加 threads=1——那是 DuckLake 单写者才需要的）。
    let _ = conn.execute_batch("PRAGMA memory_limit='4GB';");

    // ATTACH settings.json 中配置的全部数据源。
    let sources = crate::duckdb::load_data_sources();
    if !sources.is_empty() {
        if let Err(e) = crate::duckdb::attach::attach_all(&conn, &sources) {
            tracing::warn!(category = "link", "启动 ATTACH 数据源失败: {e}");
        }
    }

    let ih = conn.interrupt_handle();
    let conn_arc = Arc::new(Mutex::new(conn));
    let ih_arc = Arc::new(std::sync::Mutex::new(ih));
    tracing::info!(category = "system", "DuckDB in-memory 会话就绪，已 ATTACH {} 个数据源", sources.len());
    (Some(conn_arc), Some(ih_arc))
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
