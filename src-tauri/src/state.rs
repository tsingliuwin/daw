//! Application-wide singleton state, owned by `tauri::State`.
//!
//! Domain-agnostic machinery: the human-confirmation channel for write
//! operations, the abort flag for stopping a running stream, and the
//! current workspace path. No domain-specific state here - skills own
//! their data dependencies.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
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
    #[allow(dead_code)]
    pub workspace_dir: Arc<Mutex<PathBuf>>,
    /// The workspace key (`workspaces.path`) currently active, e.g. "DefaultProject".
    #[allow(dead_code)]
    pub workspace_path: Arc<Mutex<String>>,
    /// Write tool calls parked in "变更前确认" mode, keyed by `{task_id}:{tool_call_id}`.
    pub pending_confirmations: Arc<Mutex<HashMap<String, PendingConfirmation>>>,
    /// Aborted task IDs.
    pub aborted_tasks: Arc<Mutex<HashSet<String>>>,
    /// 搜索后端（None=未配置搜索服务，SearchTool 调时报错提示）。
    pub search_backend: Option<Arc<dyn crate::skill::search::SearchBackend>>,
    /// DuckDB in-memory 会话（可运行时更新）。安装命令成功后从 None 变 Some。
    pub duckdb: Arc<Mutex<Option<Arc<Mutex<duckdb::Connection>>>>>,
    /// DuckDB 中断句柄。
    pub interrupt_handle: Arc<Mutex<Option<Arc<std::sync::Mutex<Arc<duckdb::InterruptHandle>>>>>>,
    /// DuckDB 是否就绪（DuckLake 扩展已安装 + lake 已 ATTACH）。
    pub duckdb_ready: Arc<AtomicBool>,
}

impl AppState {
    pub fn new() -> Self {
        let ws = default_workspace_dir();
        let (duckdb_conn, interrupt_handle) = open_duckdb();
        let ready = duckdb_conn.is_some();
        Self {
            workspace_dir: Arc::new(Mutex::new(ws)),
            workspace_path: Arc::new(Mutex::new("DefaultProject".to_string())),
            pending_confirmations: Arc::new(Mutex::new(HashMap::new())),
            aborted_tasks: Arc::new(Mutex::new(HashSet::new())),
            search_backend: crate::skill::search::create_search_backend_from_settings(),
            duckdb: Arc::new(Mutex::new(duckdb_conn)),
            interrupt_handle: Arc::new(Mutex::new(interrupt_handle)),
            duckdb_ready: Arc::new(AtomicBool::new(ready)),
        }
    }
}

/// 打开 DuckDB in-memory 会话，ATTACH DuckLake + 数据源。
/// 成功返回 (Some(conn), Some(ih))，失败返回 (None, None)。
/// 安装命令 install_data_analysis_env 成功后调用此函数初始化。
pub fn open_duckdb() -> (
    Option<Arc<Mutex<duckdb::Connection>>>,
    Option<Arc<std::sync::Mutex<Arc<duckdb::InterruptHandle>>>>,
) {
    let ws_dir = match crate::db::get_aioa_dir() {
        Ok(mut p) => {
            p.push("DefaultProject");
            p
        }
        Err(e) => {
            tracing::error!(category = "system", "无法定位 aioa 目录: {e}");
            return (None, None);
        }
    };

    // in-memory 会话 + DuckLake 作为持久层（视图/表定义存 lake.sqlite，重启恢复）。
    let conn = match duckdb::Connection::open_in_memory() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(category = "system", "DuckDB 打开失败: {e}");
            return (None, None);
        }
    };
    // threads=1: DuckLake 的 SQLite catalog 是单写者，threads>1 会 "database is locked"。
    let _ = conn.execute_batch("PRAGMA memory_limit='4GB';\nPRAGMA threads=1;");

    // 加载 ducklake + sqlite 扩展，ATTACH lake 为默认 catalog。
    if let Err(e) = crate::duckdb::lake::ensure_ducklake_loaded(&conn) {
        tracing::error!(category = "system", "ducklake 加载失败: {e}");
        return (None, None);
    }
    if let Err(e) = crate::duckdb::lake::attach_workspace_lake(&conn, &ws_dir) {
        tracing::error!(category = "system", "DuckLake ATTACH 失败: {e}");
        return (None, None);
    }

    // 从 SQLite 查 DefaultProject 工作区已 link 的数据源并 ATTACH。
    // 如果 db_connections 表为空但 settings.json 有 dataSources（P1 遗留），
    // 自动迁移到表 + link 到 DefaultProject。
    let ws_path = "DefaultProject";
    let mut sources = crate::db::list_workspace_db_connections(ws_path).unwrap_or_default();
    if sources.is_empty() {
        // 迁移兼容：P1 的 settings.json dataSources → SQLite 表
        let legacy = crate::duckdb::load_data_sources();
        if !legacy.is_empty() {
            tracing::info!(category = "system", "迁移 {} 个 P1 settings.json 数据源到 SQLite", legacy.len());
            for r in &legacy {
                let _ = crate::db::create_db_connection(r);
                let _ = crate::db::link_workspace_db_connection(ws_path, &r.id);
            }
            sources = crate::db::list_workspace_db_connections(ws_path).unwrap_or_default();
        }
    }
    // 兜底修复：如果 db_connections 表有连接但 workspace_db_connections 没有 link
    // （P3 创建连接但没手动点"启用"），自动 link 到 DefaultProject。
    if sources.is_empty() {
        let all_conns = crate::db::list_db_connections().unwrap_or_default();
        if !all_conns.is_empty() {
            tracing::info!(category = "system", "自动 link {} 个未关联的数据源到 DefaultProject", all_conns.len());
            for r in &all_conns {
                let _ = crate::db::link_workspace_db_connection(ws_path, &r.id);
            }
            sources = crate::db::list_workspace_db_connections(ws_path).unwrap_or_default();
        }
    }
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
