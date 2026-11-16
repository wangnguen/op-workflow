//! Workflow steps 2-8: the task window and the step buttons on the cards.
//! Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).
//! A step runs on its own thread in the core; these commands only start, stop or read it.

use opw_domain::view::{
    ApiError, ChatView, CommentsView, DiffView, FindingDecision, FlowAction, PushCheck, TaskDetail,
};
use opw_orchestrator::AppService;
use tauri::State;

/// Everything the task window shows (reads brief.md, plan.md, push.md).
#[tauri::command(async)]
pub fn task_detail(wp_id: u64, service: State<'_, AppService>) -> Result<TaskDetail, ApiError> {
    service.task_detail(wp_id)
}

/// Run / stop / retry a step, approve or reject the plan, finish the task, …
#[tauri::command(async)]
pub fn flow_action(
    wp_id: u64,
    action: FlowAction,
    service: State<'_, AppService>,
) -> Result<TaskDetail, ApiError> {
    service.flow_action(wp_id, action)
}

/// Fix / accept / defer one review finding.
#[tauri::command(async)]
pub fn decide_finding(
    decision: FindingDecision,
    service: State<'_, AppService>,
) -> Result<TaskDetail, ApiError> {
    service.decide_finding(&decision)
}

/// Save the brief the user edited before the AI reads it.
#[tauri::command(async)]
pub fn save_brief(wp_id: u64, text: String, service: State<'_, AppService>) -> Result<TaskDetail, ApiError> {
    service.save_brief(wp_id, &text)
}

/// The Comment tab: OpenProject comments (network) and the last issue read. `mark_seen` when the
/// tab is open, so the "mới" marks and badges clear.
#[tauri::command(async)]
pub fn task_comments(
    wp_id: u64,
    mark_seen: bool,
    service: State<'_, AppService>,
) -> Result<CommentsView, ApiError> {
    service.task_comments(wp_id, mark_seen)
}

/// Rewrite the brief's OpenProject comment section with the comments there now.
#[tauri::command(async)]
pub fn refresh_brief_comments(wp_id: u64, service: State<'_, AppService>) -> Result<CommentsView, ApiError> {
    service.refresh_brief_comments(wp_id)
}

/// The Diff tab (git, read-only).
#[tauri::command(async)]
pub fn task_diff(wp_id: u64, mark_seen: bool, service: State<'_, AppService>) -> Result<DiffView, ApiError> {
    service.task_diff(wp_id, mark_seen)
}

/// Show the task folder in Explorer.
#[tauri::command]
pub fn open_task_folder(wp_id: u64, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.open_task_folder(wp_id)
}

/// The Chat tab: the transcript of one step run (`run_id` absent = the latest).
#[tauri::command(async)]
pub fn step_chat(
    wp_id: u64,
    run_id: Option<i64>,
    service: State<'_, AppService>,
) -> Result<ChatView, ApiError> {
    service.step_chat(wp_id, run_id)
}

/// "Cho phép lệnh này": add a rule to the repo's `allowBash` in config.json.
#[tauri::command(async)]
pub fn allow_bash(wp_id: u64, rule: String, service: State<'_, AppService>) -> Result<TaskDetail, ApiError> {
    service.allow_bash(wp_id, &rule)
}

/// Continue a finished run's AI session in a console window in the task's repo.
#[tauri::command]
pub fn open_resume(wp_id: u64, run_id: i64, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.open_resume(wp_id, run_id)
}

/// "Đã push xong": `git fetch` the task branch and compare it with origin (read only).
#[tauri::command(async)]
pub fn push_check(wp_id: u64, service: State<'_, AppService>) -> Result<PushCheck, ApiError> {
    service.push_check(wp_id)
}
