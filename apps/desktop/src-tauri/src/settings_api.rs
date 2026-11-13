//! Settings commands: OpenProject token (kept in Windows Credential Manager) and manual refresh.
//! Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).

use opw_domain::view::{ApiError, SettingsView};
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
