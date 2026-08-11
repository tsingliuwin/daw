//! Tauri command handlers.
//!
//! (Migrated from lakemind's `commands.rs` — the data-lake query/import/source
//! commands and the DuckDB-backed DB-connection commands were removed; the
//! domain-agnostic commands are kept: config / settings / workspaces / tasks /
//! logs / agent / human-confirmation / LLM-connection test.)
//!
//! Groups:
//! * **Config**   — get/set user settings, settings.json I/O.
//! * **Workspace / Task** — registry + per-workspace chat task persistence.
//! * **Logs**     — unified log store append/query/clear.
//! * **Agent**    — start the streaming chat, resolve a pending write, abort.

use std::path::PathBuf;

use tauri::Emitter;

use crate::db::{self};
use crate::state::AppState;

// ===========================================================================
// Config & settings commands
// ===========================================================================

#[tauri::command]
pub async fn get_app_config(key: String) -> Result<Option<String>, String> {
    let conn = db::get_db_conn()?;
    db::get_config(&conn, &key)
}

/// Write a config value.
#[tauri::command]
pub async fn set_app_config(key: String, value: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    db::set_config(&conn, &key, &value)
}

/// Read configurations from ~/.aioa/settings.json
#[tauri::command]
pub async fn load_settings_json() -> Result<String, String> {
    let mut path = db::get_aioa_dir()?;
    path.push("settings.json");
    if !path.exists() {
        return Ok("{}".to_string());
    }
    std::fs::read_to_string(path).map_err(|e| format!("读取配置文件失败: {e}"))
}

/// Write configurations to ~/.aioa/settings.json
#[tauri::command]
pub async fn save_settings_json(json: String) -> Result<(), String> {
    let mut path = db::get_aioa_dir()?;
    path.push("settings.json");
    std::fs::write(path, json).map_err(|e| format!("保存配置文件失败: {e}"))
}

/// Return the fixed system prompt (PREAMBLE) sent to the model on every call.
/// Read-only — exposes it so users can inspect what the agent is told.
#[tauri::command]
pub async fn get_system_preamble() -> Result<String, String> {
    Ok(crate::usage::PREAMBLE.to_string())
}

// ===========================================================================
// Filesystem commands
// ===========================================================================

#[derive(serde::Serialize)]
pub struct FileItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// Read the direct children of a folder. Used by the workspace file tree (the
/// OA app keeps a lighter version than lakemind's data-file tree).
#[tauri::command]
pub async fn read_directory(path: String) -> Result<Vec<FileItem>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<FileItem>, String> {
        let resolved_path = resolve_path(&path)?;
        if !resolved_path.exists() {
            return Err(format!("目录不存在: {}", resolved_path.display()));
        }

        let mut items = Vec::new();
        let entries = std::fs::read_dir(&resolved_path).map_err(|e| format!("读取目录失败: {e}"))?;
        for entry in entries {
            if let Ok(entry) = entry {
                let name = entry.file_name().to_string_lossy().to_string();
                // Hide dotfiles + the local data store.
                if name.starts_with('.') || name == "aioa.db" || name == "oa.db" {
                    continue;
                }
                let p = entry.path();
                let is_dir = p.is_dir();
                items.push(FileItem {
                    name,
                    path: p.to_string_lossy().to_string(),
                    is_dir,
                });
            }
        }
        items.sort_by(|a, b| {
            if a.is_dir != b.is_dir {
                b.is_dir.cmp(&a.is_dir)
            } else {
                a.name.cmp(&b.name)
            }
        });
        Ok(items)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

// ===========================================================================
// Workspace registry
// ===========================================================================

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Workspace {
    pub name: String,
    pub path: String,
}

#[tauri::command]
pub async fn load_workspaces() -> Result<Vec<Workspace>, String> {
    let conn = db::get_db_conn()?;
    let mut stmt = conn
        .prepare("SELECT name, path FROM workspaces ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok(Workspace { name: row.get(0)?, path: row.get(1)? }))
        .map_err(|e| e.to_string())?;
    let mut list = Vec::new();
    for r in rows {
        if let Ok(w) = r {
            list.push(w);
        }
    }
    Ok(list)
}

/// Open a native directory picker (tauri-plugin-dialog) and return the chosen
/// path, or `None` if the user cancelled. Used by the home screen's "选择新文件夹
/// 作为工作区" entry to register a new workspace.
#[tauri::command]
pub async fn select_directory(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    app.dialog()
        .file()
        .set_title("选择工作区文件夹")
        .pick_folder(move |folder| {
            let path = folder.and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().to_string());
            let _ = tx.send(path);
        });
    rx.await.map_err(|e| format!("目录选择器通道失败: {e}"))
}

#[tauri::command]
pub async fn add_workspace(name: String, path: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO workspaces (path, name, created_at) VALUES (?, ?, ?)",
        rusqlite::params![path, name, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn remove_workspace(path: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let _ = conn.execute("PRAGMA foreign_keys = ON;", []);

    // Clean up content files for all tasks under this workspace. Tasks are
    // space- and user-scoped, so each row carries its own space_id + user_id.
    let mut stmt = conn
        .prepare("SELECT id, space_id, user_id FROM tasks WHERE workspace_path = ?")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    for r in rows {
        if let Ok((id, space_id, user_id)) = r {
            delete_task_content_files(&space_id, &user_id, &id);
        }
    }

    // Deleting the workspace cascades to its tasks (FK ON DELETE CASCADE).
    conn.execute("DELETE FROM workspaces WHERE path = ?", [&path])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ===========================================================================
// Task persistence
// ===========================================================================

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    pub messages: Option<serde_json::Value>,
    pub saved: bool,
    #[serde(rename = "modelId")]
    pub model_id: Option<String>,
    #[serde(rename = "tokenUsage", skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<serde_json::Value>,
}

#[tauri::command]
pub async fn load_workspace_tasks(
    workspace_path: String,
    space_id: String,
    user_id: String,
) -> Result<Vec<Task>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<Task>, String> {
        let conn = db::get_db_conn()?;
        let mut stmt = conn
            .prepare("SELECT id, name, created_at, saved, model_id, token_usage FROM tasks WHERE workspace_path = ? AND space_id = ? AND user_id = ? ORDER BY created_at ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![&workspace_path, &space_id, &user_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i32>(3)? != 0,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        let chats_dir = db::get_chats_dir(&space_id, &user_id)?;
        let mut tasks = Vec::new();
        for r in rows {
            if let Ok((id, name, created_at, saved, model_id, token_usage_json)) = r {
                let mut messages = None;
                let filepath = chats_dir.join(format!("{id}.json"));
                if filepath.exists() {
                    let json_str = std::fs::read_to_string(filepath).unwrap_or_default();
                    messages = serde_json::from_str(&json_str).ok();
                }
                let token_usage = token_usage_json
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
                tasks.push(Task {
                    id,
                    name,
                    created_at,
                    messages,
                    saved,
                    model_id,
                    token_usage,
                });
            }
        }
        Ok(tasks)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn save_task(
    workspace_path: String,
    task_id: String,
    name: String,
    messages: serde_json::Value,
    model_id: Option<String>,
    token_usage: Option<serde_json::Value>,
    space_id: String,
    user_id: String,
    kind: Option<String>,
) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let now = now_ms();
    let usage_json = token_usage.map(|v| serde_json::to_string(&v).unwrap_or_default());
    let kind = kind.unwrap_or_else(|| "task".to_string());
    conn.execute(
        "INSERT OR REPLACE INTO tasks (id, workspace_path, name, kind, created_at, saved, model_id, token_usage, space_id, user_id)
         VALUES (?, ?, ?, ?, COALESCE((SELECT created_at FROM tasks WHERE id = ?), ?), 1, ?, ?, ?, ?)",
        rusqlite::params![task_id, workspace_path, name, kind, task_id, now, model_id, usage_json, space_id, user_id],
    )
    .map_err(|e| e.to_string())?;

    let chats_dir = db::get_chats_dir(&space_id, &user_id)?;
    let filepath = chats_dir.join(format!("{task_id}.json"));
    let json_str = serde_json::to_string(&messages).map_err(|e| e.to_string())?;
    std::fs::write(filepath, json_str).map_err(|e| format!("Failed to write chat JSON file: {e}"))?;
    Ok(())
}

#[tauri::command]
pub async fn delete_task(task_id: String, space_id: String, user_id: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    delete_task_content_files(&space_id, &user_id, &task_id);
    conn.execute("DELETE FROM tasks WHERE id = ?", [&task_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ===========================================================================
// Unified logging commands
// ===========================================================================

/// Append one log row from the frontend.
#[tauri::command]
pub async fn append_log(
    app: tauri::AppHandle,
    mut record: crate::model::LogRecord,
) -> Result<i64, String> {
    record.ts = crate::db::now_ms();
    let conn = db::get_db_conn()?;
    let id = db::insert_log(&conn, &record)?;
    let mut to_emit = record.clone();
    to_emit.id = Some(id);
    let _ = app.emit("app-log", &to_emit);
    Ok(id)
}

/// Query historical logs with optional filters. Returns newest-first.
#[tauri::command]
pub async fn query_logs(filter: crate::model::LogFilter) -> Result<Vec<crate::model::LogRecord>, String> {
    let conn = db::get_db_conn()?;
    db::query_logs(&conn, &filter)
}

/// Clear logs. `before = None` clears everything; `Some(ts)` deletes older rows.
#[tauri::command]
pub async fn clear_logs(before: Option<i64>) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    db::clear_logs(&conn, before)
}

// ===========================================================================
// Agent commands
// ===========================================================================

#[tauri::command]
pub async fn start_agent_task(
    window: tauri::Window,
    task_id: String,
    model_id: String,
    provider_id: Option<String>,
    prompt: String,
    history_json: String,
    priority: Option<String>,
    confirm_mode: Option<String>,
    kind: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let app_state = state.inner().clone();
    let priority = priority.unwrap_or_else(|| "均衡".to_string());
    let confirm_mode = confirm_mode.unwrap_or_else(|| "变更前确认".to_string());
    let scenario = crate::skill::Scenario::from_kind(&kind.unwrap_or_else(|| "task".to_string()));
    tokio::spawn(async move {
        if let Err(e) = crate::agent::run_agent_task_stream(
            window.clone(),
            task_id.clone(),
            model_id,
            provider_id,
            prompt,
            history_json,
            priority,
            confirm_mode,
            scenario,
            app_state,
        )
        .await
        {
            tracing::error!(category = "agent", "agent execution error: {e}");
            let _ = window.emit(
                "agent-event",
                crate::agent::AgentStreamEvent {
                    task_id,
                    kind: "error".to_string(),
                    text: Some(e),
                    segment: None,
                },
            );
        }
    });
    Ok(())
}

/// Resolve an OA write tool call parked in "变更前确认" mode. Called from the UI
/// when the user clicks 确认执行 (`approved = true`) or 取消 (`approved = false`).
/// The matching tool `call()` resumes via the oneshot channel.
#[tauri::command]
pub async fn resolve_tool_confirmation(
    task_id: String,
    tool_call_id: String,
    approved: bool,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    let key = format!("{}:{}", task_id, tool_call_id);
    let pending = {
        let mut map = state.pending_confirmations.lock().await;
        map.remove(&key)
    };
    match pending {
        Some(p) => {
            let _ = p.tx.send(crate::state::ConfirmDecision { approved });
            Ok(approved)
        }
        None => Err("未找到待确认的操作（可能已超时或已处理）".to_string()),
    }
}

/// Abort a running agent chat stream. Sets the abort flag so `run_stream_loop`
/// stops on the next iteration and emits "done" to unlock the frontend.
#[tauri::command]
pub async fn abort_task(task_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut aborted = state.aborted_tasks.lock().await;
    aborted.insert(task_id);
    Ok(())
}

// ===========================================================================
// LLM connection test
// ===========================================================================

/// Test an LLM provider/model against the exact config shown in the settings
/// form (not the saved settings.json), so unsaved edits are reflected. Sends a
/// minimal prompt with an 8s hard timeout and returns a friendly error on
/// failure.
#[tauri::command]
pub async fn test_llm_connection(
    endpoint: String,
    api_key: String,
    api_format: String,
    model_id: String,
) -> Result<(), String> {
    const TEST_TIMEOUT_SECS: u64 = 8;
    match tokio::time::timeout(
        std::time::Duration::from_secs(TEST_TIMEOUT_SECS),
        crate::agent::test_connection(&endpoint, &api_key, &api_format, &model_id),
    )
    .await
    {
        Ok(inner) => inner,
        Err(_elapsed) => Err(format!(
            "连接测试超时（{TEST_TIMEOUT_SECS} 秒未收到响应）。请检查网络或 Base URL 是否可访问。"
        )),
    }
}

// ===========================================================================
// Chart image export
// ===========================================================================

/// Save a base64-encoded PNG (from ECharts `getDataURL`) to a user-chosen file.
/// Called from ChartSegment's "保存图片" button.
#[tauri::command]
pub async fn save_image_from_base64(
    base64_data: String,
    default_name: String,
) -> Result<(), String> {
    let base64_str = if let Some(pos) = base64_data.find("base64,") {
        &base64_data[pos + 7..]
    } else {
        &base64_data
    };
    use base64::Engine;
    let decoded_bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_str)
        .map_err(|e| format!("Failed to decode base64 data: {e}"))?;
    let file_path = rfd::FileDialog::new()
        .set_file_name(&default_name)
        .add_filter("PNG Image", &["png"])
        .save_file();
    if let Some(path) = file_path {
        std::fs::write(&path, decoded_bytes)
            .map_err(|e| format!("Failed to write file: {e}"))?;
    }
    Ok(())
}

// ===========================================================================
// Internals — helpers
// ===========================================================================

fn delete_task_content_files(space_id: &str, user_id: &str, task_id: &str) {
    // All tasks are chat tasks in M1; the <space>/<user>/chats/ file is the
    // only content file. The chats dir may legitimately not exist yet for a
    // fresh space/user, so resolve-and-remove best-effort.
    if let Ok(dir) = db::get_chats_dir(space_id, user_id) {
        let _ = std::fs::remove_file(dir.join(format!("{task_id}.json")));
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn get_home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .ok()
}

/// Resolve a user-facing path: absolute paths pass through, `~`-relative paths
/// expand against the home dir, bare names resolve under `~/.aioa/<name>`.
fn resolve_path(workspace: &str) -> Result<PathBuf, String> {
    if workspace.starts_with("~/") || workspace == "~" {
        let mut home = get_home_dir().ok_or_else(|| "Could not find home directory".to_string())?;
        if workspace.len() > 2 {
            home.push(&workspace[2..]);
        }
        return Ok(home);
    }
    let path = PathBuf::from(workspace);
    if path.is_absolute() {
        return Ok(path);
    }
    let mut home = get_home_dir().ok_or_else(|| "Could not find home directory".to_string())?;
    home.push(".aioa");
    home.push(workspace);
    Ok(home)
}
