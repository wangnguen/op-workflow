//! Workflow steps 2-8: the task window and the step buttons on the cards.
//! Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`).
//! A step runs on its own thread in the core; these commands only start, stop or read it.

use opw_domain::view::{ApiError, FindingDecision, FlowAction, TaskDetail};
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

/// Show the task folder in Explorer.
#[tauri::command]
pub fn open_task_folder(wp_id: u64, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.open_task_folder(wp_id)
}
