//! IPC commands. Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).

use opw_domain::view::{ApiError, AppInfo, BoardSnapshot, OrbitSnapshot, Provider, UpdateView};
use opw_orchestrator::AppService;
use tauri::{AppHandle, State};

#[tauri::command]
pub fn app_info(app: AppHandle, service: State<'_, AppService>) -> AppInfo {
    service.app_info(&app.package_info().version.to_string())
}

#[tauri::command]
pub fn board_snapshot(service: State<'_, AppService>) -> BoardSnapshot {
    service.board_snapshot()
}

#[tauri::command]
pub fn orbit_snapshot(service: State<'_, AppService>) -> OrbitSnapshot {
    service.orbit_snapshot()
}

/// Cached result of the background new-version check.
#[tauri::command]
pub fn update_status(service: State<'_, AppService>) -> UpdateView {
    service.update_status()
}

/// Ask GitHub Releases now (network, so off the main thread).
#[tauri::command(async)]
pub fn check_update(service: State<'_, AppService>) -> UpdateView {
    service.check_update()
}

/// Open the newest release page of the app in the default browser.
#[tauri::command]
pub fn open_release_page(service: State<'_, AppService>) -> Result<(), ApiError> {
    service.open_release_page()
}

/// "Kết nối lại": restart the Codex app-server / run the Claude probe now (`None` = both).
#[tauri::command]
pub fn reconnect_quota(service: State<'_, AppService>, provider: Option<Provider>) {
    service.reconnect_quota(provider);
}

/// Open `claude auth login` or `codex login` in a console window.
#[tauri::command]
pub fn open_login(service: State<'_, AppService>, provider: Provider) -> Result<(), ApiError> {
    service.open_login(provider)
}
