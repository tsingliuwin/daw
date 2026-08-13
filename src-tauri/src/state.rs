//! Application-wide singleton state, owned by `tauri::State`.
//!
//! Domain-agnostic machinery: the human-confirmation channel for write
//! operations, the abort flag for stopping a running stream, and a lazily
//! created pool of per-workspace DuckDB connections. No domain-specific state
//! here - skills own their data dependencies.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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

/// One DuckDB in-memory session + its DuckLake, scoped to a single workspace.
///
/// Each workspace owns its own connection (and thus its own `USE lake` default
/// catalog and set of attached remote sources), so concurrent agent tasks in
/// different workspaces never clobber each other's session state. `busy`
/// enforces single-flight within a workspace: only one data-analysis task may
/// use a workspace's connection at a time (same-workspace tasks serialize;
/// cross-workspace tasks run in parallel).
pub struct WorkspaceConnInner {
    /// DuckDB in-memory session; DuckLake (`<ws>/.lake/lake.sqlite` + parquet)
    /// is the sole persistent layer for views/tables under this workspace.
    pub conn: Arc<Mutex<duckdb::Connection>>,
    /// Interrupt handle for query timeouts on this connection.
    pub interrupt_handle: Arc<std::sync::Mutex<Arc<duckdb::InterruptHandle>>>,
    /// Absolute path of this workspace's directory (`~/.aioa/<workspace>`).
    /// Retained for introspection/future close-hook checkpoint; not read in
    /// the tool hot path (tools get their path from the injected `WorkspaceRef`).
    #[allow(dead_code)]
    pub ws_dir: PathBuf,
    /// Single-flight guard: true while a data-analysis task is using this conn.
    pub busy: AtomicBool,
}
pub type WorkspaceConn = Arc<WorkspaceConnInner>;

/// RAII guard that clears a workspace's `busy` flag on drop, so single-flight
/// is released even on early return / error / panic. Acquired by the runner
/// after it wins the `busy` swap at task start.
pub struct BusyGuard {
    wsc: WorkspaceConn,
}
impl BusyGuard {
    pub fn new(wsc: WorkspaceConn) -> Self {
        Self { wsc }
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.wsc.busy.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// App-wide singleton: cross-tool coordination state + per-workspace DuckDB pool.
#[derive(Clone)]
pub struct AppState {
    /// Write tool calls parked in "变更前确认" mode, keyed by `{task_id}:{tool_call_id}`.
    pub pending_confirmations: Arc<Mutex<HashMap<String, PendingConfirmation>>>,
    /// Aborted task IDs.
    pub aborted_tasks: Arc<Mutex<HashSet<String>>>,
    /// Per-workspace DuckDB connections, lazily created on first use.
    /// Keyed by workspace path (e.g. "DefaultProject"). A `std::sync::Mutex`
    /// (not tokio) so it can be locked from the blocking creation path too.
    pub workspaces: Arc<std::sync::Mutex<HashMap<String, WorkspaceConn>>>,
    /// Whether the ducklake/sqlite extensions have been loaded successfully at
    /// least once (cached process-wide after the first install). Once true,
    /// any workspace's connection can be lazily created without re-downloading.
    /// `check_data_analysis_env` reports this.
    pub ext_installed: Arc<AtomicBool>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            pending_confirmations: Arc::new(Mutex::new(HashMap::new())),
            aborted_tasks: Arc::new(Mutex::new(HashSet::new())),
            workspaces: Arc::new(std::sync::Mutex::new(HashMap::new())),
            ext_installed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// Resolve a workspace's directory: `~/.aioa/<ws_path>`.
    fn workspace_dir_for(ws_path: &str) -> PathBuf {
        let mut home = get_home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.push(".aioa");
        home.push(ws_path);
        home
    }

    /// Lazily get (or create) the DuckDB connection bundle for `ws_path`.
    ///
    /// Fast path: if the workspace's connection is already cached, return it
    /// without any blocking work. Otherwise open a fresh in-memory session,
    /// load ducklake/sqlite (INSTALL if never cached), ATTACH the workspace's
    /// DuckLake + linked remote sources, cache it, and return it.
    ///
    /// Returns an error if the ducklake extension cannot be loaded — the user
    /// must click "安装数据分析环境" first (which downloads the extensions).
    pub async fn ensure_workspace_conn(&self, ws_path: &str) -> Result<WorkspaceConn, String> {
        // Fast path: already cached.
        {
            let map = self.workspaces.lock().unwrap();
            if let Some(wsc) = map.get(ws_path) {
                return Ok(wsc.clone());
            }
        }

        // Slow path: create in a blocking task (DuckDB open/LOAD/ATTACH blocks).
        let ws_path = ws_path.to_string();
        let ws_dir = Self::workspace_dir_for(&ws_path);
        let workspaces = self.workspaces.clone();
        let ext_installed = self.ext_installed.clone();
        let wsc = tokio::task::spawn_blocking(move || -> Result<WorkspaceConn, String> {
            // Re-check after acquiring the ability to create: another task may
            // have created it concurrently while we waited.
            {
                let map = workspaces.lock().unwrap();
                if let Some(wsc) = map.get(&ws_path) {
                    return Ok(wsc.clone());
                }
            }
            let conn = create_workspace_conn(&ws_path, &ws_dir)?;
            let ih = conn.interrupt_handle();
            let wsc = Arc::new(WorkspaceConnInner {
                conn: Arc::new(Mutex::new(conn)),
                interrupt_handle: Arc::new(std::sync::Mutex::new(ih)),
                ws_dir,
                busy: AtomicBool::new(false),
            });
            {
                let mut map = workspaces.lock().unwrap();
                map.insert(ws_path.clone(), wsc.clone());
            }
            ext_installed.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(wsc)
        })
        .await
        .map_err(|e| format!("线程生成失败: {e}"))??;
        Ok(wsc)
    }

    /// Get an already-cached workspace connection **without** creating one.
    /// Returns `None` if the workspace's connection hasn't been lazily created
    /// yet. Used by source link/unlink so they only (de|at)tach against
    /// already-live connections and never eagerly create one as a side effect.
    pub fn get_workspace_conn(&self, ws_path: &str) -> Option<WorkspaceConn> {
        let map = self.workspaces.lock().unwrap();
        map.get(ws_path).cloned()
    }
}

/// Open an in-memory DuckDB session, load ducklake/sqlite, ATTACH the
/// workspace's DuckLake (as default catalog), and ATTACH the workspace's linked
/// remote data sources. Used by [`AppState::ensure_workspace_conn`].
///
/// For `DefaultProject` specifically, also runs the one-time P1 legacy
/// `settings.json` migration and auto-links orphaned connections, matching the
/// previous startup bootstrap behavior.
fn create_workspace_conn(ws_path: &str, ws_dir: &Path) -> Result<duckdb::Connection, String> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| format!("DuckDB 打开失败: {e}"))?;
    let _ = conn.execute_batch("PRAGMA memory_limit='4GB';\nPRAGMA threads=1;");

    // ducklake + sqlite extensions; DuckLake is the persistent catalog backend.
    crate::duckdb::lake::ensure_ducklake_loaded(&conn)?;
    // ATTACH this workspace's DuckLake and make it the default catalog.
    crate::duckdb::lake::attach_workspace_lake(&conn, ws_dir)?;

    // ATTACH the workspace's linked remote data sources. For DefaultProject,
    // also run the one-time P1 legacy migration + auto-link bootstrap so a
    // fresh install still picks up existing data sources.
    let mut sources = crate::db::list_workspace_db_connections(ws_path).unwrap_or_default();
    if sources.is_empty() && ws_path == "DefaultProject" {
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
    if sources.is_empty() && ws_path == "DefaultProject" {
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
    tracing::info!(category = "system", "DuckDB 工作区连接就绪 ({}): 已 ATTACH {} 个数据源", ws_path, sources.len());
    Ok(conn)
}
