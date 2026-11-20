//! The Workspace (PLAN.md §12 B): Claude Code sessions the app runs, their chat and what they
//! ask. Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`). Changes of
//! a running session reach the window as the `opw://session` event (`SessionPatch`).

use opw_domain::session::{
    AskAnswer, ImageIn, NewSession, OutsideChat, RepoScript, SessionChat, SessionView, SlashCommand,
    WorkspaceSnapshot,
};
use opw_domain::view::{ApiError, DiffView};
use opw_domain::workflow::StepKind;
use opw_orchestrator::AppService;
use tauri::{AppHandle, Emitter, State};

pub const SESSION_EVENT: &str = "opw://session";

/// Forward every session change to the window.
pub fn install(app: &AppHandle, service: &AppService) {
    let handle = app.clone();
    service.set_session_sink(std::sync::Arc::new(move |patch| {
        let _ = handle.emit(SESSION_EVENT, &patch);
    }));
}

/// The tree: tasks in progress and every session with its state.
#[tauri::command(async)]
pub fn workspace_snapshot(service: State<'_, AppService>) -> WorkspaceSnapshot {
    service.workspace_snapshot()
}

/// Repos "+ Phiên mới" offers (scans the workspace folders).
#[tauri::command(async)]
pub fn workspace_repos(service: State<'_, AppService>) -> Vec<String> {
    service.workspace_repos()
}

#[tauri::command(async)]
pub fn workspace_flow(
    wp_id: u64,
    service: State<'_, AppService>,
) -> Result<opw_domain::session::WorkspaceFlow, ApiError> {
    service.workspace_flow(wp_id)
}

#[tauri::command(async)]
pub fn workspace_step_done(
    wp_id: u64,
    step: StepKind,
    service: State<'_, AppService>,
) -> Result<(), ApiError> {
    service.workspace_step_done(wp_id, step)
}

#[tauri::command(async)]
pub fn session_new(request: NewSession, service: State<'_, AppService>) -> Result<SessionView, ApiError> {
    service.session_new(&request)
}

#[tauri::command(async)]
pub fn session_chat(id: String, service: State<'_, AppService>) -> Result<SessionChat, ApiError> {
    service.session_chat(&id)
}

/// Send a message (starts or resumes the session first when nothing runs).
#[tauri::command(async)]
pub fn session_send(
    id: String,
    text: String,
    force: Option<bool>,
    images: Option<Vec<ImageIn>>,
    service: State<'_, AppService>,
) -> Result<SessionView, ApiError> {
    service.session_send(&id, &text, force.unwrap_or(false), &images.unwrap_or_default())
}

/// What the session changed (Workspace → Thay đổi).
#[tauri::command(async)]
pub fn session_diff(id: String, service: State<'_, AppService>) -> Result<DiffView, ApiError> {
    service.session_diff(&id)
}

/// Open a file of the session's repo in VS Code.
#[tauri::command(async)]
pub fn session_open_file(
    id: String,
    path: String,
    line: Option<u32>,
    service: State<'_, AppService>,
) -> Result<(), ApiError> {
    service.session_open_file(&id, &path, line)
}

/// Open a file of the task's repo in VS Code (task window → Diff).
#[tauri::command(async)]
pub fn task_open_file(
    wp_id: u64,
    path: String,
    line: Option<u32>,
    service: State<'_, AppService>,
) -> Result<(), ApiError> {
    service.task_open_file(wp_id, &path, line)
}

/// A changed file next to its version at `rev` in VS Code's diff view (Workspace → Thay đổi).
#[tauri::command(async)]
pub fn session_compare_file(
    id: String,
    rev: String,
    path: String,
    old_path: Option<String>,
    service: State<'_, AppService>,
) -> Result<(), ApiError> {
    service.session_compare_file(&id, &rev, &path, old_path.as_deref())
}

/// The same for the task window's Diff tab.
#[tauri::command(async)]
pub fn task_compare_file(
    wp_id: u64,
    rev: String,
    path: String,
    old_path: Option<String>,
    service: State<'_, AppService>,
) -> Result<(), ApiError> {
    service.task_compare_file(wp_id, &rev, &path, old_path.as_deref())
}

/// The session's repo folder in VS Code, the session staying in the app.
#[tauri::command(async)]
pub fn session_open_repo(id: String, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.session_open_repo(&id)
}

/// Repo files for `@` in the chat.
#[tauri::command(async)]
pub fn session_files(
    id: String,
    query: String,
    service: State<'_, AppService>,
) -> Result<Vec<String>, ApiError> {
    service.session_files(&id, &query)
}

/// Commands for `/` in the chat.
#[tauri::command(async)]
pub fn session_commands(id: String, service: State<'_, AppService>) -> Result<Vec<SlashCommand>, ApiError> {
    service.session_commands(&id)
}

/// The repo's package.json scripts (Shell tab).
#[tauri::command(async)]
pub fn session_scripts(id: String, service: State<'_, AppService>) -> Result<Vec<RepoScript>, ApiError> {
    service.session_scripts(&id)
}

/// A session started waiting while the user looks elsewhere: a Windows notification, when
/// those are on. A click opens the session.
#[tauri::command(async)]
pub fn session_notify_wait(id: String, app: AppHandle, service: State<'_, AppService>) {
    if let Some((title, body)) = service.session_wait_note(&id) {
        crate::toast::show_session(&app, &title, &body, &id);
    }
}

#[tauri::command(async)]
pub fn session_answer(
    id: String,
    ask_id: String,
    answer: AskAnswer,
    service: State<'_, AppService>,
) -> Result<SessionView, ApiError> {
    service.session_answer(&id, &ask_id, &answer)
}

#[tauri::command(async)]
pub fn session_interrupt(id: String, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.session_interrupt(&id)
}

#[tauri::command(async)]
pub fn session_set_mode(
    id: String,
    mode: String,
    service: State<'_, AppService>,
) -> Result<SessionView, ApiError> {
    service.session_set_mode(&id, &mode)
}

/// End the session's process here (the next message resumes it).
#[tauri::command(async)]
pub fn session_stop(id: String, service: State<'_, AppService>) -> Result<SessionView, ApiError> {
    service.session_stop(&id)
}

/// Give the session to VS Code (the app only shows it from then on).
#[tauri::command(async)]
pub fn session_open_vscode(id: String, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.session_open_vscode(&id)
}

/// Take the session off the Workspace list (its transcript stays).
#[tauri::command(async)]
pub fn session_hide(id: String, service: State<'_, AppService>) -> Result<(), ApiError> {
    service.session_hide(&id)
}

use opw_orchestrator::sessions::TerminalAction;
use opw_orchestrator::terminals::{TerminalFrame, TerminalKind};

#[tauri::command(async)]
pub fn terminal_open(
    id: String,
    kind: TerminalKind,
    view: String,
    size: (u16, u16),
    output: tauri::ipc::Channel<TerminalFrame>,
    service: State<'_, AppService>,
) -> Result<(), ApiError> {
    service.workspace_sessions()?.terminal_open(
        &id,
        kind,
        view,
        size,
        Box::new(move |frame| output.send(frame).is_ok()),
    )
}

#[tauri::command(async)]
pub fn terminal_control(
    id: String,
    kind: TerminalKind,
    view: String,
    action: TerminalAction,
    service: State<'_, AppService>,
) -> Result<(), ApiError> {
    service
        .workspace_sessions()?
        .terminal_control(&id, kind, &view, action)
}

/// The conversation of a Claude session opened outside the app (read-only).
#[tauri::command(async)]
pub fn outside_chat(id: String, service: State<'_, AppService>) -> Result<OutsideChat, ApiError> {
    service.outside_chat(&id)
}
