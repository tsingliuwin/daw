//! Daw — Data Agent Workstation: a customizable conversational workbench
//! for data and tasks.
//!
//! Entry point: wires the [`state::AppState`] singleton and the command surface
//! into the Tauri runtime. Business-level mappings (workspaces / tasks / config
//! / logs) live in the global SQLite DB (`~/.daw/daw.db`); the brand surface
//! (app name, tagline, logos, welcome copy, assistant identity) is driven by
//! the user-editable `~/.daw/brand.json` (see [`brand`]).
//!
//! (The DuckDB workspace-attach on startup and the tenets bundle seeding were
//! removed from the earlier data-lake prototype — neither exists in this app.
//! The tracing-subscriber setup and the logging-layer wiring are unchanged.)

mod agent;
mod brand;
mod commands;
mod db;
mod duckdb;
mod logging;
mod model;
mod okf;
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

    // Ensure the OKF global + default-workspace directory structure is complete.
    // Idempotent and independent of DuckDB: even if the DuckLake extension fails
    // to install, the knowledge base remains readable/writable.
    if let Err(e) = okf::Okf::production().init_all() {
        eprintln!("Failed to initialize OKF directories: {e}");
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
    // 默认显示 info，但把 rig / rig_core 的 info 压到 warn：它们在多轮流式对话时
    // 会打印大量「Depth reached / multi-turn stream finished / tool call」日志刷屏。
    // 需要排查 LLM 行为时，可用 RUST_LOG=rig=debug（或 rig_core=debug）覆盖。
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,rig=warn,rig_core=warn"));
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(logging::SqliteEmitLayer::new())
        .with(filter)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState::default())
        .setup(|_app| {
            use tauri::Manager;
            // Hand the AppHandle to the logging layer so it can emit to the
            // frontend `app-log` channel.
            logging::set_handle(_app.handle().clone());

            if let Some(window) = _app.get_webview_window("main") {
                // Window title follows the brand config (~/.daw/brand.json);
                // the value in tauri.conf.json is only the startup fallback.
                let _ = window.set_title(&brand::load_brand().app_name);

                // 按已保存的主题设置窗口底色与系统主题，避免浅色主题用户在
                // webview 加载 index.html 之前看到 tauri.conf.json 里写死的深色底。
                // 权威主题存于 config 表（ui.theme，见 src/lib/theme.ts）。
                let saved_theme = crate::db::get_db_conn()
                    .ok()
                    .and_then(|conn| crate::db::get_config(&conn, "ui.theme").ok().flatten())
                    .unwrap_or_default();
                let is_light = saved_theme == "light";
                let bg = if is_light {
                    tauri::webview::Color(0xf8, 0xfa, 0xfc, 0xff)
                } else {
                    tauri::webview::Color(0x0a, 0x0a, 0x0b, 0xff)
                };
                let _ = window.set_background_color(Some(bg));
                let _ = window.set_theme(Some(if is_light {
                    tauri::Theme::Light
                } else {
                    tauri::Theme::Dark
                }));

                #[cfg(not(target_os = "macos"))]
                {
                    let _ = window.set_decorations(false);
                }

                // 窗口在 tauri.conf.json 里配置为 visible:false，等到底色/主题都
                // 就绪后再显示，避免浅色主题用户看到「先黑色窗口底 → 再浅色 splash」。
                let _ = window.show();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_config,
            commands::set_app_config,
            commands::load_settings_json,
            commands::save_settings_json,
            commands::get_system_preamble,
            commands::get_brand_config,
            commands::get_brand_logo,
            commands::read_directory,
            commands::select_directory,
            commands::load_workspaces,
            commands::add_workspace,
            commands::remove_workspace,
            commands::load_workspace_tasks,
            commands::save_task,
            commands::update_task_meta,
            commands::delete_task,
            commands::append_log,
            commands::query_logs,
            commands::clear_logs,
            commands::start_agent_task,
            commands::resolve_tool_confirmation,
            commands::abort_task,
            commands::test_llm_connection,
            commands::save_image_from_base64,
            commands::append_chat_line,
            commands::get_db_connections,
            commands::upsert_db_connection,
            commands::delete_db_connection,
            commands::test_db_connection,
            commands::link_connection_to_workspace,
            commands::unlink_connection_from_workspace,
            commands::list_workspace_connections,
            commands::check_data_analysis_env,
            commands::install_data_analysis_env,
            commands::read_okf_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
