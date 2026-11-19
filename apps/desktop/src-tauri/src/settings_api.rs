//! Settings commands: OpenProject token (kept in Windows Credential Manager) and manual refresh.
//! Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).

use opw_domain::view::{ApiError, SettingsView, StepChoiceRequest, StepModels, TasksDirChange};
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

/// Settings → Các bước AI: agent, model and effort of one step (written to config.json).
#[tauri::command]
pub fn set_step_choice(
    service: State<'_, AppService>,
    choice: StepChoiceRequest,
) -> Result<SettingsView, ApiError> {
    service.set_step_choice(&choice)
}

#[tauri::command]
pub fn set_verify_mcp(service: State<'_, AppService>, on: bool) -> Result<SettingsView, ApiError> {
    service.set_verify_mcp(on)
}

#[tauri::command]
pub fn set_step_limits(
    service: State<'_, AppService>,
    limits: opw_domain::view::StepLimitsRequest,
) -> Result<SettingsView, ApiError> {
    service.set_step_limits(&limits)
}

/// Settings → Hạn mức: how often Claude is probed while Claude Code is in use (seconds, 0 =
/// off) and otherwise (minutes).
#[tauri::command]
pub fn set_quota_probe(
    service: State<'_, AppService>,
    active_sec: u32,
    idle_min: u32,
) -> Result<SettingsView, ApiError> {
    service.set_claude_probe(active_sec, idle_min)
}

/// Settings → Repo & nhánh → Thư viện harness of one repo (`None` = the default folder).
#[tauri::command]
pub fn set_harness_dir(
    service: State<'_, AppService>,
    repo: String,
    dir: Option<String>,
) -> Result<SettingsView, ApiError> {
    service.set_harness_dir(&repo, dir.as_deref())
}

/// Settings → Repo → Lệnh kiểm tra of one repo (empty = guessed per package).
#[tauri::command]
pub fn set_repo_checks(
    service: State<'_, AppService>,
    repo: String,
    commands: Vec<String>,
) -> Result<SettingsView, ApiError> {
    service.set_repo_checks(&repo, &commands)
}

/// Settings → Repo: forget one remembered issue repo → checkout pair.
#[tauri::command]
pub fn forget_repo_pair(
    service: State<'_, AppService>,
    pair: opw_domain::view::RepoPair,
) -> Result<SettingsView, ApiError> {
    service.forget_repo_pair(&pair)
}

/// Settings → Các bước AI: models and effort levels of the Claude / Codex CLIs on this machine.
/// Starts both CLIs (no prompt, no tokens, about 2 s), so off the UI thread.
#[tauri::command(async)]
pub fn step_models(service: State<'_, AppService>, refresh: bool) -> StepModels {
    service.step_models(refresh)
}
