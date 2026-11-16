//! Thin shell. Business logic lives in `core/`; this crate only exposes it over IPC.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// MinGW's gcc always links its own default-manifest.o next to the manifest tauri-build embeds,
// so ld prints ".rsrc merge failure: multiple non-default manifests" on every link. The
// Tauri manifest still ends up in the exe; the message is noise, so silence it for GNU only.
#![cfg_attr(all(windows, target_env = "gnu"), allow(linker_messages))]

mod alerts_api;
mod app_api;
mod flow_api;
mod settings_api;
mod start_api;
mod tray;

fn main() {
    // PTY spike from inside the app binary (PLAN.md §12): `op-workflow-desktop --pty-selftest`.
    if std::env::args().any(|a| a == "--pty-selftest") {
        let report = opw_orchestrator::selftest::run_pty();
        println!("{report}");
        if let Some(path) = opw_orchestrator::selftest::save(&report) {
            println!("saved: {}", path.display());
        }
        return;
    }
    // Live data check without opening a window: `op-workflow-desktop --live-check`.
    if std::env::args().any(|a| a == "--live-check") {
        println!(
            "{}",
            opw_orchestrator::selftest::run_live(std::time::Duration::from_secs(120))
        );
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(opw_orchestrator::AppService::start())
        .setup(|app| {
            // no tray (e.g. a shell without one) must not keep the app from opening
            if let Err(e) = tray::install(app) {
                eprintln!("tray icon unavailable: {e}");
            }
            tray::spawn_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_api::app_info,
            app_api::board_snapshot,
            app_api::orbit_snapshot,
            app_api::update_status,
            app_api::check_update,
            app_api::open_release_page,
            app_api::reconnect_quota,
            app_api::open_login,
            start_api::start_options,
            start_api::repo_state,
            start_api::fetch_repo,
            start_api::start_task,
            start_api::pause_task,
            start_api::open_brief,
            start_api::delete_branch,
            start_api::fetch_issues,
            start_api::open_browser_debug,
            settings_api::settings_status,
            settings_api::save_nx_token,
            settings_api::clear_nx_token,
            settings_api::refresh_now,
            settings_api::open_config,
            settings_api::reveal_path,
            settings_api::set_tasks_dir,
            settings_api::set_step_choice,
            settings_api::step_models,
            flow_api::task_detail,
            flow_api::flow_action,
            flow_api::decide_finding,
            flow_api::save_brief,
            flow_api::open_task_folder,
            flow_api::task_comments,
            flow_api::refresh_brief_comments,
            flow_api::task_diff,
            flow_api::step_chat,
            flow_api::allow_bash,
            flow_api::open_resume,
            alerts_api::alerts,
            alerts_api::mark_alerts_read,
            alerts_api::set_notify,
        ])
        .run(tauri::generate_context!())
        .expect("error while running op-workflow");
}
