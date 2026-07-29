//! AI OA — drive your enterprise workflows through conversation.
//!
//! Entry point: wires the [`state::AppState`] singleton and the command surface
//! into the Tauri runtime. Business-level mappings (workspaces / tasks / config
//! / logs) live in the global SQLite DB (`~/.aioa/aioa.db`); OA demo data lives
//! in `~/.aioa/oa.db`.
//!
//! (Migrated from lakemind's `lib.rs`. The DuckDB workspace-attach on startup
//! and the tenets bundle seeding were removed — neither exists in the OA app.
//! The tracing-subscriber setup and the logging-layer wiring are unchanged.)

mod agent;
mod commands;
mod db;
mod logging;
mod model;
mod skill;
mod state;
mod usage;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize the global SQLite metadata DB (workspaces / tasks / config / logs).
    if let Err(e) = db::init_global_db() {
        eprintln!("Failed to initialize central SQLite database: {e}");
    }

    // Install the tracing subscriber: the custom [`SqliteEmitLayer`] persists
    // every event to SQLite and pushes info+ to the frontend console, while the
    // fmt layer mirrors to stdout for dev diagnostics.
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_level(true)
        .compact();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(logging::SqliteEmitLayer::new())
        .with(filter)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|_app| {
            // Hand the AppHandle to the logging layer so it can emit to the
            // frontend `app-log` channel.
            logging::set_handle(_app.handle().clone());

            #[cfg(not(target_os = "macos"))]
            {
                use tauri::Manager;
                if let Some(window) = _app.get_webview_window("main") {
                    let _ = window.set_decorations(false);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_config,
            commands::set_app_config,
            commands::load_settings_json,
            commands::save_settings_json,
            commands::get_system_preamble,
            commands::read_directory,
            commands::select_directory,
            commands::load_workspaces,
            commands::add_workspace,
            commands::remove_workspace,
            commands::load_workspace_tasks,
            commands::save_task,
            commands::delete_task,
            commands::append_log,
            commands::query_logs,
            commands::clear_logs,
            commands::start_agent_task,
            commands::resolve_tool_confirmation,
            commands::abort_task,
            commands::test_llm_connection,
            commands::login_to_server,
            commands::fetch_server_models,
            commands::get_enterprises,
            commands::get_active_space,
            commands::get_current_user_id,
            commands::set_active_space,
            commands::join_enterprise,
            commands::join_enterprise_via_setup,
            commands::setup_enterprise,
            commands::leave_enterprise,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
