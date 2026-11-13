//! Settings commands: OpenProject token (kept in Windows Credential Manager) and manual refresh.
//! Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).

use opw_domain::view::{ApiError, SettingsView, TasksDirChange};
use opw_orchestrator::AppService;
use tauri::State;

#[tauri::command]
pub fn settings_status(service: State<'_, AppService>) -> SettingsView {
    service.settings()
}

/// Checks the token against OpenProject first (network), so it runs off the main thread.
#[tauri::command(async)]
pub fn save_nx_token(
    base_url: String,
    token: String,
    service: State<'_, AppService>,
) -> Result<SettingsView, ApiError> {
    service.save_nx_token(&base_url, &token)
}

#[tauri::command]
pub fn clear_nx_token(service: State<'_, AppService>) -> Result<SettingsView, ApiError> {
    service.clear_nx_token()
}

#[tauri::command]
pub fn refresh_now(service: State<'_, AppService>) {
    service.refresh_now();
}

/// Show config.json in Explorer.
#[tauri::command]
pub fn open_config(service: State<'_, AppService>) -> Result<(), ApiError> {
    service.open_config()
}

/// Show a folder or file listed in Settings in Explorer.
#[tauri::command]
pub fn reveal_path(service: State<'_, AppService>, path: String) -> Result<(), ApiError> {
    service.reveal_path(&path)
}

/// Put the task folders (briefs) in `dir` (`None` = back to the data folder) and move the old
/// ones there. Disk work, so off the UI thread.
#[tauri::command(async)]
pub fn set_tasks_dir(
    service: State<'_, AppService>,
    dir: Option<String>,
) -> Result<TasksDirChange, ApiError> {
    service.set_tasks_dir(dir.as_deref())
}
