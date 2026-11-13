//! IPC view DTOs. The JSON shape is the contract with `apps/desktop/ui/src/ipc/types.ts`:
//! camelCase keys, ISO 8601 timestamps as strings, `None` fields omitted.

use serde::{Deserialize, Serialize};

use crate::finding::{FindingState, Severity};
use crate::workflow::{StepKind, StepState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    Waiting,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskColumn {
    Inbox,
    Active,
    AwaitingPush,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateKind {
    PlanReview,
    MediumTriage,
    AgentQuestion,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuotaWindowKind {
    FiveHour,
    SevenDay,
}

/// Stable error shape at the core ↔ UI boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub core: String,
}

// ---------- quota ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub kind: QuotaWindowKind,
    /// 0..100
    pub used_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub provider: Provider,
    pub windows: Vec<QuotaWindow>,
    pub source: String,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// Connection to one AI provider's quota source (Codex app-server, Claude probe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    Connecting,
    Ok,
    Error,
    /// The CLI is not on PATH.
    Missing,
    /// The CLI says nobody is signed in (or the sign-in expired).
    SignedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLink {
    pub provider: Provider,
    pub status: LinkStatus,
    /// What went wrong and what the app does next (Vietnamese, shown as is).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// When the status last changed.
    pub since: String,
}

// ---------- board ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRef {
    pub kind: StepKind,
    pub index: u8,
    pub state: StepState,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCard {
    pub id: String,
    pub wp_id: u64,
    pub title: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub nx_status: String,
    /// `#rrggbb` the OpenProject admin gave this status; absent when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_status_color: Option<String>,
    pub column: TaskColumn,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<StepRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_pct: Option<f64>,
    pub new_comments: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodayStats {
    pub tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSnapshot {
    pub tasks: Vec<TaskCard>,
    /// Tasks in "Đang làm", most recent first. Several can run at once.
    pub active_task_ids: Vec<String>,
    pub quota: Vec<QuotaSnapshot>,
    /// Quota connections per provider; empty in demo data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ProviderLink>,
    pub today: TodayStats,
    /// `demo` until an OpenProject token is saved, then `live`.
    pub mode: DataMode,
    /// Problem to show above the board (e.g. OpenProject unreachable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// Briefs still live in the hidden data folder and the user has not chosen: offer this
    /// folder (on the Desktop) once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks_dir_offer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DataMode {
    Demo,
    Live,
}

// ---------- settings ----------

/// What the Settings dialog shows. Never contains the token itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub nx_base_url: String,
    pub nx_token_set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_task_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_last_sync: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_quota_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_quota_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_error: Option<String>,
    /// Folders scanned for repos (`workspaces` in config).
    pub workspaces: Vec<String>,
    /// What "Bắt đầu làm việc" remembered per OpenProject project.
    pub projects: Vec<ProjectMemory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    /// Browser that allows remote debugging right now ("Chrome", "Edge"); `None` = not enabled
    /// (or the browser is closed). Needed for "Lấy issue".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser_debug: Option<String>,
    /// Folder of the task folders (`<task id>/brief.md`) in use now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks_dir: Option<String>,
    /// `false` until the user picked a folder (or kept the default) once.
    pub tasks_dir_chosen: bool,
    /// The Desktop folder the app suggests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks_dir_suggested: Option<String>,
}

/// Result of moving the briefs to another folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksDirChange {
    pub path: String,
    /// Task folders moved from the old place.
    pub moved: u32,
    /// Folders that stayed behind, and why.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemory {
    pub name: String,
    pub repos: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

// ---------- start work ----------

/// A git repo the user can pick in the "Bắt đầu làm việc" dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoOption {
    /// Absolute path; also the key sent back in [`StartRequest`].
    pub path: String,
    /// Folder name, e.g. `acme-web`.
    pub name: String,
    /// Remote `origin`, shown to tell same-named folders apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Remote matches an issue link in the task description.
    pub suggested: bool,
    /// Already used for this project before.
    pub remembered: bool,
}

/// Everything the dialog needs before a repo is picked. Built without network calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartOptions {
    pub wp_id: u64,
    pub task_id: String,
    pub title: String,
    pub project: String,
    pub nx_status: String,
    /// Suggested first, then remembered, then the rest by name.
    pub repos: Vec<RepoOption>,
    /// Repo to pre-select (used before for this task, issue match, or remembered).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Branch name to pre-fill.
    pub branch: String,
    /// Where `branch` came from: `saved` (this task was started before), `project` (template
    /// remembered for the project) or `default`. Only `default` may be replaced by the
    /// convention guessed from the picked repo.
    pub branch_source: String,
    /// Issue links found in the task description.
    pub issue_links: Vec<String>,
    /// Folders scanned for repos.
    pub workspaces: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchOption {
    /// As git shows it: `development` or `origin/development`.
    pub name: String,
    pub remote: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<String>,
    /// e.g. `ahead 2`, `behind 14`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// Main branch (main, development, the remote's default, ...): never offered for deletion.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub protected: bool,
}

/// Delete one branch from the branch list (the user picked it and confirmed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteBranchRequest {
    pub repo: String,
    /// `f/WP-1` or `origin/f/WP-1`.
    pub branch: String,
    /// delete `origin/<branch>` instead of the local branch
    pub remote: bool,
    /// local only: also drop commits that are not merged anywhere (`git branch -D`)
    #[serde(default)]
    pub force: bool,
    /// to refresh the suggested branch name of the dialog's task
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wp_id: Option<u64>,
}

/// Branches and working-tree state of one repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoState {
    pub path: String,
    pub name: String,
    /// Newest commit first.
    pub branches: Vec<BranchOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// `git status --porcelain` lines (capped).
    pub dirty: Vec<String>,
    pub dirty_count: usize,
    /// Base picked last time in this repo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_base: Option<String>,
    /// Branch name from the repo's own convention (guessed from its branches), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    pub wp_id: u64,
    pub repo: String,
    pub base: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResult {
    pub task_id: String,
    pub branch: String,
    /// `false` when the branch already existed and the app only switched to it.
    pub created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brief_path: Option<String>,
    /// Non-fatal problems (brief built without comments, config not saved, …); the branch is
    /// ready anyway.
    pub warnings: Vec<String>,
    /// GitHub issue links found in the task; the UI then runs "Lấy issue" by itself.
    #[serde(default)]
    pub issue_links: u32,
}

/// "Lấy issue": issues read through the user's Chrome into the brief's `## Từ issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueFetch {
    pub read: u32,
    /// Issues that could not be read (the others are in the brief).
    pub warnings: Vec<String>,
}

// ---------- update check ----------

/// New-version check against GitHub Releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateView {
    pub current: String,
    pub enabled: bool,
    /// Newest release found, e.g. `0.2.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// `latest` is newer than `current`.
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------- orbit ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    pub kind: StepKind,
    pub index: u8,
    pub state: StepState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentView {
    pub name: String,
    /// "running" | "done"
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvent {
    pub at: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentView {
    pub id: String,
    pub kind: AgentKind,
    pub status: SessionStatus,
    pub step: StepKind,
    pub context_tokens: u64,
    pub context_window: u64,
    pub handoff_at: u64,
    pub compactions: u32,
    pub subagents: Vec<SubagentView>,
    pub children: Vec<AgentView>,
    pub recent_tools: Vec<ToolEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingView {
    pub fingerprint: String,
    pub severity: Severity,
    pub title: String,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub source: String,
    pub state: FindingState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateView {
    pub kind: GateKind,
    pub since: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSystem {
    pub task_id: String,
    pub title: String,
    pub repo: String,
    pub branch: String,
    pub nx_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_status_color: Option<String>,
    pub started_at: String,
    pub steps: Vec<StepView>,
    pub agents: Vec<AgentView>,
    pub findings: Vec<FindingView>,
    pub new_comments: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub task_id: String,
    pub title: String,
    pub project: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistantSession {
    pub id: String,
    pub agent: AgentKind,
    pub label: String,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrbitSnapshot {
    /// The most recent task in "Đang làm"; the map draws it in the centre by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<TaskSystem>,
    /// The other tasks in "Đang làm", most recent first; the UI can swap one into the centre.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<TaskSystem>,
    pub inbox: Vec<InboxItem>,
    pub distant: Vec<DistantSession>,
    pub quota: Vec<QuotaSnapshot>,
    /// Why the inbox may be empty (loading, OpenProject error, URL missing), as on the board.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Locks the JSON contract with `types.ts`.
    #[test]
    fn task_card_serializes_camel_case_and_omits_none() {
        let card = TaskCard {
            id: "WP-187716".into(),
            wp_id: 187716,
            title: "t".into(),
            project: "Web Platform".into(),
            repo: None,
            branch: Some("feature/187716".into()),
            nx_status: "In progress".into(),
            nx_status_color: Some("#cc5de8".into()),
            column: TaskColumn::AwaitingPush,
            step: Some(StepRef {
                kind: StepKind::CrossReview,
                index: 6,
                state: StepState::WaitingUser,
                since: "2026-11-13T08:00:00Z".into(),
            }),
            agent: Some(AgentKind::Codex),
            cost_usd: None,
            context_pct: Some(24.0),
            new_comments: 2,
        };
        let v = serde_json::to_value(&card).unwrap();
        assert_eq!(
            v,
            json!({
                "id": "WP-187716", "wpId": 187716, "title": "t", "project": "Web Platform",
                "branch": "feature/187716", "nxStatus": "In progress", "nxStatusColor": "#cc5de8", "column": "awaitingPush",
                "step": {"kind": "crossReview", "index": 6, "state": "waitingUser", "since": "2026-11-13T08:00:00Z"},
                "agent": "codex", "contextPct": 24.0, "newComments": 2
            })
        );
    }

    #[test]
    fn provider_link_serializes_for_ts() {
        let link = ProviderLink {
            provider: Provider::Codex,
            status: LinkStatus::SignedOut,
            message: None,
            since: "2026-11-13T08:00:00Z".into(),
        };
        assert_eq!(
            serde_json::to_value(&link).unwrap(),
            json!({"provider": "codex", "status": "signedOut", "since": "2026-11-13T08:00:00Z"})
        );
    }

    #[test]
    fn start_types_match_ts() {
        let req: StartRequest = serde_json::from_value(json!({
            "wpId": 188001, "repo": "C:/code/acme-web", "base": "origin/development", "branch": "f/WP-188001"
        }))
        .unwrap();
        assert_eq!(req.wp_id, 188001);
        let state = RepoState {
            path: "C:/code/acme-web".into(),
            name: "acme-web".into(),
            branches: vec![BranchOption {
                name: "origin/development".into(),
                remote: true,
                committed_at: None,
                track: None,
                protected: true,
            }],
            current: Some("development".into()),
            dirty: vec![],
            dirty_count: 0,
            last_base: None,
            suggested_branch: Some("f/WP-188001".into()),
        };
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            json!({"path": "C:/code/acme-web", "name": "acme-web",
                   "branches": [{"name": "origin/development", "remote": true, "protected": true}],
                   "current": "development", "dirty": [], "dirtyCount": 0,
                   "suggestedBranch": "f/WP-188001"})
        );
    }

    #[test]
    fn enums_match_ts_literals() {
        let s = |v: Value| v.as_str().unwrap().to_owned();
        assert_eq!(
            s(serde_json::to_value(GateKind::MediumTriage).unwrap()),
            "mediumTriage"
        );
        assert_eq!(
            s(serde_json::to_value(QuotaWindowKind::FiveHour).unwrap()),
            "fiveHour"
        );
        assert_eq!(
            s(serde_json::to_value(StepKind::PushSummary).unwrap()),
            "pushSummary"
        );
        assert_eq!(
            s(serde_json::to_value(StepKind::FinalReview).unwrap()),
            "finalReview"
        );
        assert_eq!(s(serde_json::to_value(Severity::Critical).unwrap()), "critical");
        assert_eq!(
            s(serde_json::to_value(SessionStatus::Waiting).unwrap()),
            "waiting"
        );
    }
}
