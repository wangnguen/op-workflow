//! The Workspace (PLAN.md §12 B): Claude Code sessions the app runs, their chat and what they
//! ask. Names and payloads must match `apps/desktop/ui/src/ipc/types.ts` (`Commands`). Changes of
//! a running session reach the window as the `opw://session` event (`SessionPatch`).

use opw_domain::session::{AskAnswer, NewSession, SessionChat, SessionView, WorkspaceSnapshot};
use opw_domain::view::ApiError;
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
    service: State<'_, AppService>,
) -> Result<SessionView, ApiError> {
    service.session_send(&id, &text, force.unwrap_or(false))
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
