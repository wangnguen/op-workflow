//! The main window's X (PLAN.md §12 part 5d): ask, go to the tray (the AI keeps running) or quit
//! (every running AI step is stopped together with what it started, then the app exits). What
//! to do comes from `onClose` in config; "ask" lets the UI show its dialog.
//! Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).

use std::time::Duration;

use opw_domain::view::{ApiError, CloseChoice, SettingsView};
use opw_orchestrator::AppService;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// The UI shows "Đóng op-workflow?".
pub const CLOSE_ASKED_EVENT: &str = "opw://close-asked";
/// Longest wait for the AI steps to stop before the app exits anyway.
const QUIT_WAIT: Duration = Duration::from_secs(15);
/// On exits that did not go through [`quit`] (Windows signing out): a short last try.
pub const EXIT_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloseAsked {
    /// AI steps running now.
    running: usize,
}

/// The user pressed X (or Alt+F4) on the main window; the close itself was already prevented.
pub fn on_close_requested(app: &AppHandle) {
    let service = app.state::<AppService>();
    match service.close_choice() {
        CloseChoice::Ask => {
            let _ = app.emit(
                CLOSE_ASKED_EVENT,
                CloseAsked {
                    running: service.running_steps(),
                },
            );
        }
        CloseChoice::Tray => to_tray(app),
        CloseChoice::Quit => quit(app),
    }
}

/// Hide the window; the app, its tray icon and the AI steps keep running.
pub fn to_tray(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        // without a tray icon nothing could bring a hidden window back
        if app.tray_by_id(crate::tray::TRAY_ID).is_some() {
            let _ = w.hide();
        } else {
            let _ = w.minimize();
        }
    }
}

/// "Thoát hẳn": the window goes away at once, the running AI steps are stopped (each process
/// with everything it started), then the app exits.
pub fn quit(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("opw-quit".into())
        .spawn(move || {
            app.state::<AppService>().quit_flow(QUIT_WAIT);
            app.exit(0);
        });
}

/// The close dialog's answer; `remember` saves it as `onClose` (changed back in Settings).
#[tauri::command]
pub fn close_window(
    app: AppHandle,
    service: State<'_, AppService>,
    choice: CloseChoice,
    remember: bool,
) -> Result<(), ApiError> {
    if remember && choice != CloseChoice::Ask {
        service.set_on_close(choice)?;
    }
    match choice {
        CloseChoice::Tray => to_tray(&app),
        CloseChoice::Quit => quit(&app),
        CloseChoice::Ask => {}
    }
    Ok(())
}

/// Settings → Thông báo → "Khi đóng cửa sổ".
#[tauri::command]
pub fn set_on_close(service: State<'_, AppService>, choice: CloseChoice) -> Result<SettingsView, ApiError> {
    service.set_on_close(choice)
}
