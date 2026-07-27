//! Application-wide singleton state, owned by `tauri::State`.
//!
//! (Migrated from lakemind's `state.rs` with all DuckDB fields removed. What
//! remains is the domain-agnostic machinery that the OA app needs too: the
//! human-confirmation channel for write operations, the abort flag for
//! stopping a running stream, and the current workspace path.)
//!
//! ## What was removed
//!
//! lakemind held an in-memory DuckDB connection here (`conn`,
//! `interrupt_handle`, `sources` cache). The OA app has no DuckDB — business
//! data lives in the OA backend ([`crate::oa::backend::OaBackend`]), which is
//! stored here as a trait object so it can be swapped for a real OA API
//! adapter later.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::sync::oneshot;

use crate::db::get_home_dir;
use crate::oa::backend::OaBackend;
use crate::oa::backend::LocalOaBackend;

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

/// App-wide singleton: the OA backend + the cross-tool coordination state.
#[derive(Clone)]
pub struct AppState {
    /// The OA backend (local demo by default; swappable for a real API adapter).
    /// Cloning clones the inner `Arc`, so every tool clone shares one backend.
    pub oa_backend: Arc<dyn OaBackend>,
    /// Absolute path of the workspace directory currently active.
    ///
    /// M2 entry point: OA tools that touch workspace-scoped data (linked OA
    /// systems, per-workspace approval routing) will read this. Unused in M1.
    #[allow(dead_code)]
    pub workspace_dir: Arc<Mutex<PathBuf>>,
    /// The workspace key (`workspaces.path`) currently active, e.g. "DefaultProject".
    ///
    /// M2 entry point: same as `workspace_dir` — keyed per-workspace OA lookups.
    #[allow(dead_code)]
    pub workspace_path: Arc<Mutex<String>>,
    /// Write tool calls parked in "变更前确认" mode, keyed by `{task_id}:{tool_call_id}`.
    /// Each entry holds a oneshot sender that resumes the blocked tool once the
    /// user approves or cancels from the UI (via `resolve_tool_confirmation`).
    pub pending_confirmations: Arc<Mutex<HashMap<String, PendingConfirmation>>>,
    /// Aborted task IDs. Inserted by `abort_chat`; checked by `run_stream_loop`
    /// each iteration so a long-running stream stops promptly.
    pub aborted_tasks: Arc<Mutex<HashSet<String>>>,
}

impl AppState {
    pub fn new() -> Self {
        let ws = default_workspace_dir();
        Self {
            oa_backend: Arc::new(LocalOaBackend::new()),
            workspace_dir: Arc::new(Mutex::new(ws)),
            workspace_path: Arc::new(Mutex::new("DefaultProject".to_string())),
            pending_confirmations: Arc::new(Mutex::new(HashMap::new())),
            aborted_tasks: Arc::new(Mutex::new(HashSet::new())),
        }
    }
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
