//! IPC commands. Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).

use opw_domain::view::{AppInfo, BoardSnapshot, OrbitSnapshot};
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
