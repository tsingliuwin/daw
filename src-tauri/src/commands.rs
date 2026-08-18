//! Tauri command handlers.
//!
//! (The data-lake query/import/source
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
use crate::okf;
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

/// Read configurations from ~/.daw/settings.json
#[tauri::command]
pub async fn load_settings_json() -> Result<String, String> {
    let mut path = db::get_app_dir()?;
    path.push("settings.json");
    if !path.exists() {
        return Ok("{}".to_string());
    }
    std::fs::read_to_string(path).map_err(|e| format!("读取配置文件失败: {e}"))
}

/// Write configurations to ~/.daw/settings.json
#[tauri::command]
pub async fn save_settings_json(json: String) -> Result<(), String> {
    let mut path = db::get_app_dir()?;
    path.push("settings.json");
    std::fs::write(path, json).map_err(|e| format!("保存配置文件失败: {e}"))
}

/// Return the system prompt the agent receives (brand-resolved). Read-only —
/// exposes it so users can inspect what the agent is told.
#[tauri::command]
pub async fn get_system_preamble() -> Result<String, String> {
    Ok(crate::usage::general_preamble(
        &crate::brand::load_brand().app_name,
    ))
}

/// Effective brand config from `~/.daw/brand.json` (Daw defaults otherwise).
#[tauri::command]
pub async fn get_brand_config() -> Result<crate::brand::BrandConfig, String> {
    Ok(crate::brand::load_brand())
}

/// Custom logo (`kind` = "light" | "dark") as a base64 data URI; `None` when
/// no custom file is configured and the frontend should use the built-in one.
#[tauri::command]
pub async fn get_brand_logo(kind: String) -> Result<Option<String>, String> {
    crate::brand::load_logo(&kind)
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
/// app keeps a lighter version than the data-lake prototype's data-file tree).
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
                // Hide dotfiles + the local data store (daw.db; oa.db kept for migrated legacy dirs).
                if name.starts_with('.') || name == "daw.db" || name == "oa.db" {
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

    // Ensure the new workspace has the standard OKF directory skeleton + git
    // versioning (idempotent, non-fatal). `path` is the user-picked absolute
    // folder; init_workspace creates the okf/ subtree under it. Knowledge is
    // filled in later as it is discovered.
    match okf::paths::resolve_workspace_dir(&path) {
        Ok(d) => {
            let ws_str = d.to_string_lossy().to_string();
            if let Err(e) = okf::Okf::production().init_workspace(&ws_str) {
                tracing::warn!(category = "system", "新建工作区 OKF 初始化失败 ({path}): {e}");
            }
        }
        Err(e) => tracing::warn!(category = "system", "解析新工作区目录失败 ({path}): {e}"),
    }
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
    #[serde(rename = "kind", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
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
            .prepare("SELECT id, name, created_at, saved, model_id, token_usage, kind FROM tasks WHERE workspace_path = ? AND space_id = ? AND user_id = ? ORDER BY created_at ASC")
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
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        let chats_dir = db::get_chats_dir(&space_id, &user_id)?;
        let mut tasks = Vec::new();
        for r in rows {
            if let Ok((id, name, created_at, saved, model_id, token_usage_json, kind)) = r {
                let mut messages = None;
                let jsonl_path = chats_dir.join(format!("{id}.jsonl"));
                if jsonl_path.exists() {
                    // 读 JSONL：逐行解析，按 msg_id 去重取最后一条，保持首次出现顺序。
                    let content = std::fs::read_to_string(&jsonl_path).unwrap_or_default();
                    let mut first_order: Vec<String> = Vec::new();
                    let mut last_val: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
                    for line in content.lines() {
                        if line.trim().is_empty() { continue; }
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                            if let Some(msg_id) = v.get("id").and_then(|i| i.as_str()) {
                                let msg_id = msg_id.to_string();
                                if !last_val.contains_key(&msg_id) {
                                    first_order.push(msg_id.clone());
                                }
                                last_val.insert(msg_id, v);
                            }
                        }
                    }
                    let arr: Vec<serde_json::Value> = first_order.into_iter()
                        .filter_map(|id| last_val.remove(&id))
                        .collect();
                    messages = Some(serde_json::Value::Array(arr));
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
                    kind,
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

    // 写 JSONL：每行一个 ChatMessage。
    let chats_dir = db::get_chats_dir(&space_id, &user_id)?;
    let filepath = chats_dir.join(format!("{task_id}.jsonl"));
    let arr = messages.as_array().ok_or("messages 不是数组")?;
    let mut file = std::fs::File::create(&filepath).map_err(|e| format!("创建文件失败: {e}"))?;
    use std::io::Write;
    for msg in arr {
        let line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        writeln!(file, "{line}").map_err(|e| format!("写入失败: {e}"))?;
    }
    Ok(())
}

/// 只更新 tasks 表的元数据（name/kind/saved/modelId/tokenUsage），
/// **不触碰 .jsonl 内容文件**。与 save_task 的区别：save_task 会截断并重写
/// .jsonl，而本命令仅 upsert 元数据行，供"用户消息已增量 append 落盘、
/// 只需同步元数据"的场景使用，避免 save_task 误清空内容文件。
#[tauri::command]
pub async fn update_task_meta(
    workspace_path: String,
    task_id: String,
    name: String,
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
    Ok(())
}

/// 追加一行 JSON 到 .jsonl 文件（流式输出时实时落盘）。
#[tauri::command]
pub async fn append_chat_line(
    task_id: String,
    line: serde_json::Value,
    space_id: String,
    user_id: String,
) -> Result<(), String> {
    let chats_dir = db::get_chats_dir(&space_id, &user_id)?;
    let filepath = chats_dir.join(format!("{task_id}.jsonl"));
    let line_str = serde_json::to_string(&line).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true).append(true).open(&filepath)
        .map_err(|e| format!("打开文件失败: {e}"))?;
    use std::io::Write;
    writeln!(file, "{line_str}").map_err(|e| format!("写入失败: {e}"))?;
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
                    attempt: None,
                    max_attempts: None,
                    delay_secs: None,
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
// Data analysis environment management
// ===========================================================================

/// 检查数据分析环境是否就绪。
///
/// 本进程内已确认过安装，或磁盘上已有上次安装的扩展（LOAD 探测成功）时返回
/// true——启动后无需再点「启用」。只 LOAD 不 INSTALL，绝不联网下载；只有扩展
/// 确实未安装时才返回 false，前端据此展示首次安装引导。
#[tauri::command]
pub async fn check_data_analysis_env(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    if state.ext_installed.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(true);
    }
    let ext_installed = state.ext_installed.clone();
    let ready = tokio::task::spawn_blocking(move || {
        let Ok(conn) = duckdb::Connection::open_in_memory() else {
            return false;
        };
        crate::duckdb::try_load_extensions(&conn)
    })
    .await
    .unwrap_or(false);
    if ready {
        ext_installed.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(ready)
}

/// 直接读取 OKF 知识库文件内容（不经过 agent，供前端内链点击跳转）。
#[tauri::command]
pub async fn read_okf_file(
    ws_path: String,
    category: String,
    name: String,
    heading: String,
) -> Result<String, String> {
    let cat = crate::okf::model::Category::from_str(&category)
        .ok_or_else(|| format!("未知知识类别: {category}"))?;
    let result = tokio::task::spawn_blocking(move || {
        crate::okf::Okf::production().read(&ws_path, cat, &name, &heading)
    }).await
    .map_err(|e| format!("线程生成失败: {e}"))?;
    match result {
        Ok(o) => Ok(o.content),
        Err(e) => Err(e),
    }
}

/// 安装数据分析环境（DuckLake + sqlite 扩展 + ATTACH lake）。
/// 逐步发 "ducklake-install" 事件给前端展示进度。
#[tauri::command]
pub async fn install_data_analysis_env(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use tauri::Emitter;
    // 如果已就绪（扩展已安装且 DefaultProject 连接已缓存），直接返回。
    if state.ext_installed.load(std::sync::atomic::Ordering::Relaxed)
        && state.workspaces.lock().unwrap().contains_key("DefaultProject")
    {
        let _ = window.emit("ducklake-install", serde_json::json!({ "step": "done", "message": "已就绪" }));
        return Ok(());
    }

    let ws_dir = match crate::db::get_app_dir() {
        Ok(mut p) => { p.push("DefaultProject"); p }
        Err(e) => return Err(format!("无法定位工作区目录: {e}")),
    };
    let state_inner = state.inner().clone();

    tokio::spawn(async move {
        let window = window;
        let state = state_inner;

        let result = {
            let window_inner = window.clone();
                tokio::task::spawn_blocking(move || -> Result<crate::state::WorkspaceConn, String> {
                    use std::sync::Arc;
                let conn = duckdb::Connection::open_in_memory()
                    .map_err(|e| format!("DuckDB 打开失败: {e}"))?;
                let _ = conn.execute_batch("PRAGMA memory_limit='4GB';\nPRAGMA threads=1;");

                // Step 1: INSTALL ducklake
                let _ = window_inner.emit("ducklake-install", serde_json::json!({
                    "step": "install_ducklake", "message": "正在下载 ducklake 扩展…"
                }));
                if conn.execute("LOAD ducklake;", []).is_err() {
                    if let Err(e) = conn.execute("INSTALL ducklake;", []) {
                        tracing::warn!(category = "duckdb", "INSTALL ducklake failed: {e}");
                    }
                    conn.execute("LOAD ducklake;", [])
                        .map_err(|e| format!("ducklake 扩展加载失败: {e}"))?;
                }

                // Step 2: INSTALL sqlite
                let _ = window_inner.emit("ducklake-install", serde_json::json!({
                    "step": "install_sqlite", "message": "正在下载 sqlite 扩展…"
                }));
                if conn.execute("LOAD sqlite;", []).is_err() {
                    if let Err(e) = conn.execute("INSTALL sqlite;", []) {
                        tracing::warn!(category = "duckdb", "INSTALL sqlite failed: {e}");
                    }
                    conn.execute("LOAD sqlite;", [])
                        .map_err(|e| format!("sqlite 扩展加载失败: {e}"))?;
                }

                // Step 3: ATTACH lake
                let _ = window_inner.emit("ducklake-install", serde_json::json!({
                    "step": "attach_lake", "message": "正在初始化数据湖…"
                }));
                crate::duckdb::lake::attach_workspace_lake(&conn, &ws_dir)?;

                // Step 4: ATTACH 外部数据源
                let _ = window_inner.emit("ducklake-install", serde_json::json!({
                    "step": "attach_sources", "message": "正在连接数据源…"
                }));
                let ws_path = "DefaultProject";
                let sources = crate::db::list_workspace_db_connections(ws_path).unwrap_or_default();
                if !sources.is_empty() {
                    if let Err(e) = crate::duckdb::attach::attach_all(&conn, &sources) {
                        tracing::warn!(category = "link", "ATTACH 数据源失败: {e}");
                    }
                }

                let ih = conn.interrupt_handle();
                let wsc = Arc::new(crate::state::WorkspaceConnInner {
                    conn: Arc::new(tokio::sync::Mutex::new(conn)),
                    interrupt_handle: Arc::new(std::sync::Mutex::new(ih)),
                    ws_dir,
                    busy: std::sync::atomic::AtomicBool::new(false),
                });
                Ok(wsc)
            }).await
        };

        match result {
            Ok(Ok(wsc)) => {
                // 插入 per-workspace 连接池（DefaultProject）+ 标记扩展已安装。
                {
                    let mut map = state.workspaces.lock().unwrap();
                    map.insert("DefaultProject".to_string(), wsc);
                }
                state.ext_installed.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = window.emit("ducklake-install", serde_json::json!({
                    "step": "done", "message": "数据分析环境已就绪"
                }));
            }
            Ok(Err(e)) => {
                tracing::error!(category = "system", "数据分析环境安装失败: {e}");
                let _ = window.emit("ducklake-install", serde_json::json!({
                    "step": "error", "message": e
                }));
            }
            Err(e) => {
                let msg = format!("线程生成失败: {e}");
                tracing::error!(category = "system", "数据分析环境安装失败: {msg}");
                let _ = window.emit("ducklake-install", serde_json::json!({
                    "step": "error", "message": msg
                }));
            }
        }
    });
    Ok(())
}

// ===========================================================================
// Data source (db_connections) management
// ===========================================================================

#[tauri::command]
pub async fn get_db_connections() -> Result<Vec<crate::model::DataSourceConfig>, String> {
    crate::db::list_db_connections()
}

#[tauri::command]
pub async fn upsert_db_connection(
    config: crate::model::DataSourceConfig,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if config.id.is_empty() || config.name.trim().is_empty() {
        return Err("连接 ID 和名称不能为空".to_string());
    }
    // upsert：存在则 update，不存在则 create + 自动 link 到 DefaultProject。
    let existing = crate::db::get_db_connection(&config.id)?;
    let is_new = existing.is_none();
    if existing.is_some() {
        crate::db::update_db_connection(&config)?;
    } else {
        let mut rec = config.clone();
        rec.created_at = crate::db::now_ms();
        crate::db::create_db_connection(&rec)?;
    }

    // 新建连接自动 link 到 DefaultProject；若该工作区连接已存在则即时 ATTACH。
    if is_new {
        let ws_path = "DefaultProject".to_string();
        crate::db::link_workspace_db_connection(&ws_path, &config.id)?;
        if let Some(wsc) = state.get_workspace_conn(&ws_path) {
            let dc = wsc.conn.clone();
            let rec = config.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let guard = dc.blocking_lock();
                crate::duckdb::attach::attach_one(&guard, &rec)
            }).await;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_db_connection(id: String) -> Result<(), String> {
    crate::db::delete_db_connection(&id)?;
    Ok(())
}

/// 测试数据源连接（不影响主会话）。open_in_memory + ATTACH + DETACH 验证。
#[tauri::command]
pub async fn test_db_connection(config: crate::model::DataSourceConfig) -> Result<String, String> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| format!("打开测试连接失败: {e}"))?;
    crate::duckdb::attach::attach_one(&conn, &config)
        .map(|_| {
            // 成功后 DETACH 清理。
            let alias = crate::duckdb::attach::workspace_attach_alias(&config.name);
            let _ = conn.execute(&format!("DETACH {alias};"), []);
            "连接成功".to_string()
        })
}

/// link 数据源到工作区（持久化 + 立即 ATTACH 到主会话，失败则回滚 link）。
#[tauri::command]
pub async fn link_connection_to_workspace(
    ws_path: String,
    conn_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // 1. 持久化 link
    crate::db::link_workspace_db_connection(&ws_path, &conn_id)?;
    // 2. 立即 ATTACH 到该工作区的连接（若已存在）；连接未创建则跳过——
    //    首次任务启动时 lazy attach_all 会补上。
    let conn_record = crate::db::get_db_connection(&conn_id)?
        .ok_or_else(|| "数据源不存在".to_string())?;
    if let Some(wsc) = state.get_workspace_conn(&ws_path) {
        let dc = wsc.conn.clone();
        let rec = conn_record.clone();
        let alias = crate::duckdb::attach::workspace_attach_alias(&rec.name);
        let result = tokio::task::spawn_blocking(move || {
            let guard = dc.blocking_lock();
            // 先检查 catalog 是否已存在（启动时可能已 ATTACH）
            let check_sql = format!("SELECT count(*) FROM duckdb_databases() WHERE database_name = '{}'", alias);
            let exists: i64 = guard.query_row(&check_sql, [], |r| r.get(0)).unwrap_or(0);
            if exists > 0 {
                return Ok(()); // 已 ATTACH，跳过
            }
            crate::duckdb::attach::attach_one(&guard, &rec)
        }).await
        .map_err(|e| format!("线程生成失败: {e}"))?;
        if let Err(e) = result {
            // ATTACH 失败 → 回滚 link，保证 UI truthful
            let _ = crate::db::unlink_workspace_db_connection(&ws_path, &conn_id);
            return Err(format!("ATTACH 失败（已回滚）: {e}"));
        }
    }
    Ok(())
}

/// unlink 数据源（删 link + DETACH）。
#[tauri::command]
pub async fn unlink_connection_from_workspace(
    ws_path: String,
    conn_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    crate::db::unlink_workspace_db_connection(&ws_path, &conn_id)?;
    // DETACH 从该工作区的连接（若已存在）
    let conn_record = crate::db::get_db_connection(&conn_id)?;
    if let (Some(wsc), Some(rec)) = (state.get_workspace_conn(&ws_path), conn_record) {
        let dc = wsc.conn.clone();
        let name = rec.name.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let guard = dc.blocking_lock();
            crate::duckdb::attach::detach_one(&guard, &name)
        }).await;
    }
    Ok(())
}

/// 列出某工作区已 link 的数据源。
#[tauri::command]
pub async fn list_workspace_connections(
    ws_path: String,
) -> Result<Vec<crate::model::DataSourceConfig>, String> {
    crate::db::list_workspace_db_connections(&ws_path)
}

// ===========================================================================
// Internals — helpers
// ===========================================================================

fn delete_task_content_files(space_id: &str, user_id: &str, task_id: &str) {
    if let Ok(dir) = db::get_chats_dir(space_id, user_id) {
        let _ = std::fs::remove_file(dir.join(format!("{task_id}.jsonl")));
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
/// expand against the home dir, bare names resolve under `~/.daw/<name>`.
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
    Ok(db::get_app_dir()?.join(workspace))
}
