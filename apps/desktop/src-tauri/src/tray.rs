//! System tray icon (quota gauge) and the background loop that turns new alerts into Windows
//! notifications (`toast.rs`). The picture, the tooltip and what counts as an alert come from the
//! core (`AppService::tray_view`, `AppService::poll_alerts`); this file only hands them to Windows.

use std::time::Duration;

use opw_orchestrator::AppService;
use opw_orchestrator::tray::ICON_SIZE;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Emitter, Manager};

pub(crate) const TRAY_ID: &str = "quota";
/// Event the UI listens to: open the quota popover.
pub const OPEN_QUOTA_EVENT: &str = "opw://open-quota";
/// How often new alerts and the tray picture are checked.
const TICK: Duration = Duration::from_secs(3);

/// Bring the main window to the front. Windows does not let a window take the foreground from
/// another app on its own (a window opened from a terminal stays behind it, only its taskbar
/// button blinks), so it is put on top for a moment and then back to normal.
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_always_on_top(true);
        let _ = w.set_always_on_top(false);
        let _ = w.set_focus();
    }
}

pub fn install(app: &App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Mở op-workflow", true, None::<&str>)?;
    let quota = MenuItem::with_id(app, "quota", "Xem hạn mức", true, None::<&str>)?;
    let reconnect = MenuItem::with_id(app, "reconnect", "Đọc lại hạn mức", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Thoát op-workflow", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &quota, &reconnect, &sep, &quit])?;
    let view = app.state::<AppService>().tray_view();
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::new_owned(view.rgba, ICON_SIZE, ICON_SIZE))
        .tooltip(&view.tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quota" => {
                show_main(app);
                let _ = app.emit(OPEN_QUOTA_EVENT, ());
            }
            "reconnect" => app.state::<AppService>().reconnect_quota(None),
            // like "Thoát hẳn": running AI steps stop first
            "quit" => crate::window_api::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                show_main(app);
                let _ = app.emit(OPEN_QUOTA_EVENT, ());
            }
        })
        .build(app)?;
    Ok(())
}

/// Every few seconds: show new alerts as Windows notifications and redraw the tray gauge when
/// the quota moved.
pub fn spawn_watcher(app: AppHandle) {
    let _ = std::thread::Builder::new()
        .name("opw-alerts".into())
        .spawn(move || {
            let mut last_key = String::new();
            loop {
                std::thread::sleep(TICK);
                let service = app.state::<AppService>();
                for alert in service.poll_alerts() {
                    crate::toast::show(&app, &alert);
                }
                let view = service.tray_view();
                if view.key != last_key {
                    if let Some(tray) = app.tray_by_id(TRAY_ID) {
                        let _ = tray.set_icon(Some(Image::new_owned(view.rgba, ICON_SIZE, ICON_SIZE)));
                        let _ = tray.set_tooltip(Some(&view.tooltip));
                    }
                    last_key = view.key;
                }
            }
        });
}
