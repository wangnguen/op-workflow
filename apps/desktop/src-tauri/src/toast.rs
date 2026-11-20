//! Windows notifications for the bell's alerts. On Windows they are shown with
//! `tauri-winrt-notification` directly, because the notification plugin (through notify-rust)
//! never reports a click there: here a click brings the app to the front and the UI opens what
//! the alert is about (the task window on its tab, or the quota popover).

use opw_domain::view::AlertView;
use tauri::AppHandle;

/// Event the UI listens to: a notification was clicked, open what it is about.
#[cfg(windows)]
pub const OPEN_ALERT_EVENT: &str = "opw://open-alert";

/// Show one alert as a notification.
#[cfg(windows)]
pub fn show(app: &AppHandle, alert: &AlertView) {
    use tauri::Emitter;
    use tauri_winrt_notification::Toast;

    let handle = app.clone();
    let clicked = alert.clone();
    let shown = Toast::new(&app_id(app))
        .title(&alert.title)
        .text1(&alert.body)
        // runs on a Windows thread while the app is open; a click in the notification centre
        // after the app has closed does nothing (there is no COM activator to start it)
        .on_activated(move |_| {
            crate::tray::show_main(&handle);
            let _ = handle.emit(OPEN_ALERT_EVENT, &clicked);
            Ok(())
        })
        .show();
    if let Err(e) = shown {
        eprintln!("notification failed: {e}");
    }
}

/// Event the UI listens to: a session's notification was clicked, open that session.
pub const OPEN_SESSION_EVENT: &str = "opw://open-session";

/// A Workspace session waits for the user: a click brings the app up on that session.
#[cfg(windows)]
pub fn show_session(app: &AppHandle, title: &str, body: &str, id: &str) {
    use tauri::Emitter;
    use tauri_winrt_notification::Toast;

    let handle = app.clone();
    let id = id.to_owned();
    let shown = Toast::new(&app_id(app))
        .title(title)
        .text1(body)
        .on_activated(move |_| {
            crate::tray::show_main(&handle);
            let _ = handle.emit(OPEN_SESSION_EVENT, &id);
            Ok(())
        })
        .show();
    if let Err(e) = shown {
        eprintln!("notification failed: {e}");
    }
}

#[cfg(not(windows))]
pub fn show_session(app: &AppHandle, title: &str, body: &str, _id: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Show one alert as a notification (no click handling outside Windows).
#[cfg(not(windows))]
pub fn show(app: &AppHandle, alert: &AlertView) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title(&alert.title)
        .body(&alert.body)
        .show();
}

/// The id Windows files the notification under: the installed app's own id (its name and icon
/// show in the notification), or PowerShell's for a build run from `target\debug` /
/// `target\release` (`pnpm tauri dev`), which has no Start-menu shortcut carrying an id. Same
/// rule as the notification plugin.
#[cfg(windows)]
fn app_id(app: &AppHandle) -> String {
    use tauri_winrt_notification::Toast;

    let from_target = tauri::utils::platform::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        .is_some_and(|dir| dir.ends_with("target/debug") || dir.ends_with("target/release"));
    if from_target {
        Toast::POWERSHELL_APP_ID.to_owned()
    } else {
        app.config().identifier.clone()
    }
}
