//! The Workspace (PLAN.md §4, §12 B): Claude Code sessions the app runs the way the VS Code
//! extension does, and what their chat shows. Same JSON contract as `view` (camelCase, `None`
//! omitted); mirrored in `apps/desktop/ui/src/ipc/types.ts`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::view::{ChatItem, FlowError, GateKind};
use crate::workflow::{StepKind, StepState};

/// What a session is for. Later parts add the separate review, harness and browser sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    /// A task's session (the newest one is the task's current session).
    Main,
    /// A session in a repo, without a task ("+ Phiên mới").
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionState {
    /// Nothing running: the next message starts or resumes the session.
    Idle,
    /// A turn is running (or the session is starting).
    Running,
    /// A turn waits for the user: a permission, a question, a plan, folder trust.
    Waiting,
    /// Open in VS Code or a terminal: the app only shows it.
    Outside,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    /// Claude Code's session id (the transcript's file name).
    pub id: String,
    pub kind: SessionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wp_id: Option<u64>,
    /// The task's display id (`PDA-187716`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub title: String,
    /// The folder the session runs in.
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub state: SessionState,
    /// The app runs a process for it now.
    #[serde(default)]
    pub live: bool,
    /// The interactive terminal owns this Claude session. Chat follows the transcript.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f64>,
    /// Where it is open while the app only shows it: "VS Code", "terminal", …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outside: Option<String>,
    /// Permission mode in effect (as the CLI reported it), else the one chosen for the next start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The model the CLI reported it runs with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model picked in the chat for this session; `None` = the CLI's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_pick: Option<String>,
    pub cost_usd: f64,
    /// The session has a transcript (a message was sent at least once).
    #[serde(default)]
    pub started: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Why the last start or run failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What it did last, in one line (the last tool it ran or the start of its last answer),
    /// for the sessions table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// A copy of another conversation that has not run yet: the id it was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_of: Option<String>,
    /// The running CLI's background tasks (subagents, commands run in the background), while
    /// they run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<BackgroundTask>,
    /// Stop was asked; the turn or the background tasks are ending.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopping: bool,
}

/// One background task of a session's CLI, as its `system` events describe it
/// (`background_tasks_changed`, `task_started`, `task_progress`; CLI 2.1.329).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTask {
    /// The CLI's task id (what `stop_task` takes).
    pub id: String,
    /// `agent` (a subagent), `bash` (a command run in the background) or the CLI's own name.
    pub kind: String,
    pub description: String,
    /// The tool call that started it (an Agent call shows its steps in the chat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// When the app saw it start (RFC 3339).
    pub started_at: String,
    /// What it does now (the CLI's progress line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_uses: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// Stop was asked for this one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopping: bool,
}

/// The end of what a background task wrote (`tasks/<id>.output` of the session; commands run
/// in the background write there, subagents leave it empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutput {
    pub text: String,
    /// Bytes left out at the start.
    #[serde(default)]
    pub cut: u64,
    /// The file, when it was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// A task in progress, shown in the tree even before it has a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTask {
    pub wp_id: u64,
    pub task_id: String,
    pub title: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFlow {
    /// The task's main session, where its steps go (the newest one after Làm lại từ đầu).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub enabled: bool,
    pub auto: bool,
    pub running: Option<StepKind>,
    pub session_step: bool,
    pub gate: Option<GateKind>,
    pub error: Option<FlowError>,
    pub steps: Vec<WorkspaceStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStep {
    pub step: StepKind,
    pub enabled: bool,
    pub state: StepState,
    /// Why it cannot run now (nothing to review yet, no finding picked to fix).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub tasks: Vec<WorkspaceTask>,
    /// Newest first.
    pub sessions: Vec<SessionView>,
    /// Claude sessions open on this machine that the app does not run (empty when the user
    /// hid them). Newest first.
    #[serde(default)]
    pub outside: Vec<OutsideSession>,
    /// Sessions taken off the list ("Đã lưu trữ"), newest first: the app's and those opened
    /// outside it.
    #[serde(default)]
    pub archived: Vec<ArchivedSession>,
}

/// A session taken off the Workspace list. Its conversation stays with Claude Code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedSession {
    pub id: String,
    pub title: String,
    /// The folder it runs in.
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Opened outside the app (VS Code, a terminal) rather than run by it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside: bool,
    pub archived_at: String,
}

/// A Claude session opened outside the app (VS Code, a terminal), read from Claude Code's
/// status file and transcript. The app only shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideSession {
    pub id: String,
    /// The folder it runs in.
    pub cwd: String,
    pub title: String,
    /// Idle, running or waiting (never `outside`: all of these are).
    pub state: SessionState,
    /// Where it is open: VS Code, terminal, …
    pub open_in: String,
    /// When its status last changed (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// The conversation of a session opened outside the app, read from its transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideChat {
    pub session: OutsideSession,
    pub items: Vec<ChatItem>,
    /// Older items left out.
    pub dropped: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AskKind {
    /// A tool call needs approval.
    Permission,
    /// AskUserQuestion.
    Question,
    /// ExitPlanMode: approve the plan or ask for another.
    Plan,
    /// Claude Code has no trust record for the folder (asked before the session starts).
    Trust,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestion {
    pub question: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub header: String,
    pub options: Vec<AskOption>,
    #[serde(default)]
    pub multi_select: bool,
}

/// Something the session waits for the user to answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAsk {
    /// The CLI's request id (or `trust`).
    pub id: String,
    pub kind: AskKind,
    /// Tool name; empty for folder trust.
    pub tool: String,
    /// What the call works on: the command, the path, the folder.
    pub summary: String,
    /// The CLI's description of the call, or why it asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The CLI suggested a rule, so "always allow" can be offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<AskQuestion>,
    /// The plan (markdown) to approve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// The user's answer to a [`SessionAsk`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AskAnswer {
    /// Run the call; `always` also adds the rule the CLI suggested for this session.
    Allow {
        #[serde(default)]
        always: bool,
    },
    /// Refuse the call; `message` tells the AI why.
    Deny {
        #[serde(default)]
        message: Option<String>,
    },
    /// AskUserQuestion: question → chosen label(s) or the user's own text.
    Answers { answers: BTreeMap<String, String> },
    /// Approve the plan and switch to `mode` (`auto`, `acceptEdits`, `default`).
    ApprovePlan {
        #[serde(default)]
        mode: Option<String>,
    },
    /// Keep planning; `feedback` goes to the AI.
    RejectPlan {
        #[serde(default)]
        feedback: Option<String>,
    },
    /// Folder trust: `true` records trust (as the extension's "Trust folder"); `false` runs the
    /// session with the user's own settings only.
    Trust { trust: bool },
}

/// Everything the chat pane shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChat {
    pub session: SessionView,
    pub items: Vec<ChatItem>,
    /// Older items left out.
    #[serde(default)]
    pub dropped: u32,
    /// Text the AI is writing right now (not a finished message yet).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub partial: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<SessionAsk>,
    /// Grows with every change; a patch with `seq` ≤ the chat's is old.
    pub seq: u64,
}

/// A change of a session the app runs, pushed as it happens: `items` replace the chat's items
/// from index `from` on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPatch {
    pub id: String,
    pub seq: u64,
    pub from: u32,
    pub items: Vec<ChatItem>,
    #[serde(default)]
    pub dropped: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub partial: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<SessionAsk>,
    pub session: SessionView,
}

/// "+ Phiên mới" or a task's first message: the session of task `wp_id`, or one in `repo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSession {
    #[serde(default)]
    pub wp_id: Option<u64>,
    #[serde(default)]
    pub repo: Option<String>,
}

/// An image pasted into the chat, sent with the message (base64, as the API takes it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageIn {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub media_type: String,
    pub data: String,
}

/// A script of the repo's package.json, run in the session's shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoScript {
    pub name: String,
    /// What package.json runs for it.
    pub command: String,
    /// What to type in the shell: `pnpm run dev`.
    pub run: String,
}

/// A command the chat's `/` offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Where it comes from: `claude`, `repo`, `user`, `skill`, `plugin`.
    pub source: String,
}

/// Permission modes the chat offers (the CLI's names).
pub const MODES: [&str; 4] = ["default", "acceptEdits", "plan", "auto"];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn answers_parse_from_the_ui() {
        let a: AskAnswer = serde_json::from_value(json!({"kind": "allow", "always": true})).unwrap();
        assert_eq!(a, AskAnswer::Allow { always: true });
        let a: AskAnswer = serde_json::from_value(json!({"kind": "deny"})).unwrap();
        assert_eq!(a, AskAnswer::Deny { message: None });
        let a: AskAnswer =
            serde_json::from_value(json!({"kind": "answers", "answers": {"Màu?": "Xanh"}})).unwrap();
        assert!(matches!(a, AskAnswer::Answers { answers } if answers["Màu?"] == "Xanh"));
        let a: AskAnswer = serde_json::from_value(json!({"kind": "approvePlan", "mode": "auto"})).unwrap();
        assert_eq!(
            a,
            AskAnswer::ApprovePlan {
                mode: Some("auto".into())
            }
        );
        let a: AskAnswer = serde_json::from_value(json!({"kind": "trust", "trust": false})).unwrap();
        assert_eq!(a, AskAnswer::Trust { trust: false });
    }

    #[test]
    fn views_are_camel_case_without_empty_fields() {
        let ask = SessionAsk {
            id: "r1".into(),
            kind: AskKind::Permission,
            tool: "Bash".into(),
            summary: "npm test".into(),
            detail: None,
            always: None,
            questions: Vec::new(),
            plan: None,
        };
        assert_eq!(
            serde_json::to_value(&ask).unwrap(),
            json!({"id": "r1", "kind": "permission", "tool": "Bash", "summary": "npm test"})
        );
        let n: NewSession = serde_json::from_value(json!({"wpId": 7})).unwrap();
        assert_eq!(
            n,
            NewSession {
                wp_id: Some(7),
                repo: None
            }
        );
    }
}
