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

/// Download the newer version's installer now (again after a failure), in the background.
#[tauri::command]
pub fn download_update(service: State<'_, AppService>) -> UpdateView {
    service.download_update()
}

/// "Cài bản mới": start the checked installer (it closes the app, installs without questions and
/// opens the app again), then quit the way "Thoát hẳn" does. `force` = install although AI work
/// runs (the UI asked).
#[tauri::command]
pub fn install_update(
    app: AppHandle,
    service: State<'_, AppService>,
    force: Option<bool>,
) -> Result<(), ApiError> {
    service.install_update(force.unwrap_or(false))?;
    crate::window_api::quit(&app);
    Ok(())
}

/// Settings → Giới thiệu → "Tự tải bản mới".
#[tauri::command(async)]
pub fn set_update_auto_download(service: State<'_, AppService>, on: bool) -> Result<UpdateView, ApiError> {
    service.set_update_auto_download(on)
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

/// Claude Code and Codex on this machine (starts both CLIs, so off the main thread).
#[tauri::command(async)]
pub fn ai_clis(service: State<'_, AppService>) -> Vec<opw_domain::view::AiCli> {
    service.ai_clis()
}

/// "Cài": the CLI's installer in a console.
#[tauri::command]
pub fn install_cli(service: State<'_, AppService>, provider: Provider) -> Result<(), ApiError> {
    service.install_cli(provider)
}

/// A link in a brief or a chat (https only), in the default browser.
#[tauri::command]
pub fn open_link(service: State<'_, AppService>, url: String) -> Result<(), ApiError> {
    service.open_link(&url)
}
