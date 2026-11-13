//! "Bắt đầu làm việc" commands: repo/branch choices, create the branch locally, pause.
//! Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).
//! Everything that runs git, scans folders or calls OpenProject is `async` (off the UI thread).

use opw_domain::view::{
    ApiError, DeleteBranchRequest, IssueFetch, RepoState, StartOptions, StartRequest, StartResult,
};
use opw_orchestrator::AppService;
use tauri::State;

#[tauri::command(async)]
pub fn start_options(wp_id: u64, service: State<'_, AppService>) -> Result<StartOptions, ApiError> {
    service.start_options(wp_id)
}

#[tauri::command(async)]
pub fn repo_state(
    repo: String,
    wp_id: Option<u64>,
    service: State<'_, AppService>,
) -> Result<RepoState, ApiError> {
    service.repo_state(&repo, wp_id)
}

#[tauri::command(async)]
pub fn fetch_repo(
    repo: String,
    wp_id: Option<u64>,
    service: State<'_, AppService>,
) -> Result<RepoState, ApiError> {
    service.fetch_repo(&repo, wp_id)
}

/// Delete a branch the user confirmed (local, or on origin).
#[tauri::command(async)]
pub fn delete_branch(
    request: DeleteBranchRequest,
    service: State<'_, AppService>,
) -> Result<RepoState, ApiError> {
    service.delete_branch(&request)
}

#[tauri::command(async)]
pub fn start_task(request: StartRequest, service: State<'_, AppService>) -> Result<StartResult, ApiError> {
    service.start_task(&request)
}

#[tauri::command]
pub fn pause_task(wp_id: u64, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.pause_task(wp_id)
}

#[tauri::command]
pub fn open_brief(wp_id: u64, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.open_brief(wp_id)
}

/// "Lấy issue" through the user's Chrome; waits for the Allow click and the pages.
#[tauri::command(async)]
pub fn fetch_issues(wp_id: u64, service: State<'_, AppService>) -> Result<IssueFetch, ApiError> {
    service.fetch_issues(wp_id)
}

/// Open `chrome://inspect/#remote-debugging` in Chrome.
#[tauri::command]
pub fn open_browser_debug(service: State<'_, AppService>) -> Result<(), ApiError> {
    service.open_browser_debug()
}
