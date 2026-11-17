//! The bell and the notification switches.
//! Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).

use opw_domain::view::{AlertView, ApiError, NotifyKind, SettingsView};
use opw_orchestrator::AppService;
use tauri::State;

/// The bell (kept across restarts), newest first.
#[tauri::command]
pub fn alerts(service: State<'_, AppService>) -> Vec<AlertView> {
    service.alerts()
}

/// Mark alerts read; an empty list marks all.
#[tauri::command]
pub fn mark_alerts_read(ids: Vec<u64>, service: State<'_, AppService>) {
    service.mark_alerts_read(&ids);
}

/// Turn one kind of Windows notification on or off (`notify` in config.json).
#[tauri::command]
pub fn set_notify(
    kind: NotifyKind,
    on: bool,
    service: State<'_, AppService>,
) -> Result<SettingsView, ApiError> {
    service.set_notify(kind, on)
}
