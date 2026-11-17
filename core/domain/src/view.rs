//! IPC view DTOs. The JSON shape is the contract with `apps/desktop/ui/src/ipc/types.ts`:
//! camelCase keys, ISO 8601 timestamps as strings, `None` fields omitted.

use serde::{Deserialize, Serialize};

use crate::finding::{FindingState, Severity};
use crate::workflow::{AfterFix, StepKind, StepState};

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

/// The task's priority on OpenProject (read-only), shown as OpenProject shows it: its name in
/// the colour the admin gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorityView {
    /// As OpenProject shows it, e.g. `High`.
    pub name: String,
    /// `#rrggbb` when the OpenProject admin gave the priority a colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Place in OpenProject's own order of priorities, 1 = lowest; with `ranks` (how many there
    /// are) it tells how urgent the task is, e.g. the map draws higher ones stronger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranks: Option<u8>,
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
    /// A review reported findings: the user picks which ones the AI fixes. Saved tasks from
    /// before call it `mediumTriage` (the final review's medium findings only).
    #[serde(alias = "mediumTriage")]
    Findings,
    /// A Sửa lỗi finished: run the cross review again, or go on.
    AfterFix,
    AgentQuestion,
    Error,
    /// The user chose to code this task by hand at the plan gate.
    SelfCode,
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
    /// What the task waits for, when it waits for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateKind>,
    /// One line about the workflow (the error of a failed step, the next step, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_note: Option<String>,
    /// The failed step needs this provider signed in again ("Đăng nhập" on the card).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<Provider>,
    /// How the last "Lấy issue" went (kept across restarts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues: Option<IssueStatus>,
    /// "Đã xong" column: when the user marked it done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// "Đã xong" column: the push check made when it was marked done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_check: Option<PushCheck>,
    /// "Đã xong" column: OpenProject still lists the task as open (not Resolved yet).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nx_open: bool,
    /// The task's priority on OpenProject; absent when unknown (e.g. no longer listed there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<PriorityView>,
}

/// Is the task branch on origin? Made by "Đã push xong" (`git fetch`, then local branch vs
/// `origin/<branch>`). Read only: the app never pushes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushCheck {
    pub branch: String,
    /// origin has the branch and every local commit of it.
    pub pushed: bool,
    /// origin has the branch at all.
    pub remote: bool,
    /// Local commits origin does not have.
    pub ahead: u32,
    /// Commits on origin the local branch does not have.
    pub behind: u32,
    /// Files changed but not committed (only counted while the repo is on the task branch).
    pub uncommitted: u32,
    /// Short hash of the local branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// `git fetch` failed: the result compares with what origin had at the last fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_error: Option<String>,
    /// The repo is on another branch right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_branch: Option<String>,
    pub checked_at: String,
    /// The user marked the task done although the check said it was not pushed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forced: bool,
}

/// The last "Lấy issue" of a task: when, how many issues made it into the brief, or what failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueStatus {
    pub at: String,
    pub read: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    /// Alerts (bell) the user has not opened yet.
    #[serde(default)]
    pub unread_alerts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DataMode {
    Demo,
    Live,
}

// ---------- settings ----------

/// What the Settings dialog shows. Never contains the token itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Which Windows notifications are on (`notify` in config).
    #[serde(default)]
    pub notify: NotifySettings,
    /// Agent, model and effort of each AI step, with what past runs cost (Các bước AI).
    #[serde(default)]
    pub steps: Vec<StepSetting>,
    /// Probe Claude this often (seconds) while Claude Code is in use outside the app; 0 = off
    /// (`quota.claudeActiveProbeSec`).
    #[serde(default)]
    pub claude_active_probe_sec: u32,
    /// Probe Claude this often (minutes) when Claude Code is not in use (`quota.claudeProbeMin`).
    #[serde(default)]
    pub claude_probe_min: u32,
}

/// Models and effort levels the Claude and Codex CLIs on this machine offer, for Settings →
/// Các bước AI. Read on demand (`step_models`), not with every settings poll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepModels {
    pub claude: AgentCatalog,
    pub codex: AgentCatalog,
}

/// What one agent CLI offers. When the CLI is missing or does not answer, `from_cli` is
/// `false`, `error` says why and `models` is the last list the CLI gave on this machine (`read_at`
/// says when), or empty when there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCatalog {
    pub agent: AgentKind,
    pub models: Vec<AgentModel>,
    /// The model the CLI uses when the app passes no flag ("Theo máy"), as configured
    /// (e.g. `opus[1m]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// The entry of `models` that `default_model` resolves to, when the list has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_id: Option<String>,
    /// `default_model` the way the list names it ("Opus 5.5 · 1M").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_label: Option<String>,
    /// The effort the CLI uses when the app passes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    pub from_cli: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When the list was read (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_at: Option<String>,
}

/// One model in an agent's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModel {
    /// What goes into `--model` / `-m`.
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Effort levels this model takes; empty = it takes no effort flag.
    #[serde(default)]
    pub efforts: Vec<String>,
    /// The model's own default effort (Codex lists one per model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// The CLI's recommended / default model.
    #[serde(default)]
    pub is_default: bool,
}

/// One AI step in Settings → Các bước AI. The app never picks a model or effort by itself:
/// `None` means the CLI's default on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepSetting {
    pub step: StepKind,
    pub agent: AgentKind,
    /// The agent of this step can be switched (only cross-review for now).
    pub agent_choice: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Effort used when none is picked, if the app passes one (push summary: `low`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_default: Option<String>,
    /// Finished runs of this step over every task.
    pub runs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_seconds: Option<f64>,
    /// Model of the latest finished run (what "theo máy" turned out to be).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,
}

/// Settings → Các bước AI: what the user picked for one step (`None` = the machine's default).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepChoiceRequest {
    pub step: StepKind,
    #[serde(default)]
    pub agent: Option<AgentKind>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

/// Windows notifications by kind; the bell lists every alert either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotifySettings {
    /// New comments on OpenProject, a task moved to Test failed / Reopened.
    pub comments: bool,
    /// A step finished, waits for you or failed.
    pub steps: bool,
    /// A quota window passed a warning level (`quota.warnAt`).
    pub quota: bool,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            comments: true,
            steps: true,
            quota: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotifyKind {
    Comments,
    Steps,
    Quota,
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
    /// Starting again a task that finished a round: where to carry on (as "Làm tiếp"). `None`
    /// = stay where it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_on: Option<CarryOn>,
}

/// "Làm tiếp" from the start dialog: run again from `step` (one the task reached), with a
/// note for Viết code; no step = carry on where the task stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarryOn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<StepKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// Comments newer than the previous read (0 on the first read).
    #[serde(default)]
    pub new_comments: u32,
}

// ---------- comments ----------

/// One comment in the Comment tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentView {
    /// OpenProject activity id, or `<issue url>#<n>` for an issue comment.
    pub id: String,
    pub created_at: String,
    /// Writer's name, OpenProject only (issue comments are read without names).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub text: String,
    /// The user wrote it (OpenProject: author = `users/me`; GitHub: `viewerDidAuthor`). Shown
    /// folded, never new.
    #[serde(default)]
    pub mine: bool,
    /// Someone else wrote it after the user last opened the Comment tab ("cần xem").
    pub is_new: bool,
    /// Where to read it in the browser: the comment on its OpenProject work package, or the
    /// GitHub issue (issue comments are read without their ids). Opened via `open_task_link`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A GitHub issue as the last "Lấy issue" read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueThread {
    pub url: String,
    pub title: String,
    /// Newest first.
    pub comments: Vec<CommentView>,
    /// Comments GitHub counts that the page did not give.
    pub unread: u32,
}

/// The Comment tab: OpenProject comments (read now) and the issues of the last "Lấy issue".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentsView {
    /// Newest first.
    pub comments: Vec<CommentView>,
    /// Why the OpenProject comments could not be read (no token, network, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_url: Option<String>,
    pub issues: Vec<IssueThread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues_read_at: Option<String>,
    /// OpenProject has comments newer than the brief's comment section.
    pub brief_stale: bool,
    pub fetched_at: String,
}

// ---------- diff ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffChange {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLineView {
    pub kind: DiffLineKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunkView {
    /// The function or class git names after `@@` (may be empty).
    pub section: String,
    pub lines: Vec<DiffLineView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFileView {
    /// Repo-relative, with `/`.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub change: DiffChange,
    pub binary: bool,
    /// Not added to git yet.
    pub untracked: bool,
    pub added: u32,
    pub removed: u32,
    /// Lines were left out (very long file or diff).
    pub truncated: bool,
    /// Its change differs from when the user last opened the Diff tab.
    pub changed: bool,
    pub hunks: Vec<DiffHunkView>,
}

/// The Diff tab: the task's change since it left its base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffView {
    pub base: String,
    /// Short hash of the fork point.
    pub fork_point: String,
    pub files: Vec<DiffFileView>,
    pub added: u32,
    pub removed: u32,
    /// Files whose change differs from the last look (0 the first time).
    pub changed_since_seen: u32,
    /// The user opened the Diff tab before (otherwise nothing counts as changed).
    pub seen_before: bool,
    /// E.g. the repo is on another branch, so only the task's commits are shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub truncated: bool,
}

// ---------- alerts ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertKind {
    /// New comments on OpenProject.
    Comment,
    /// The task moved to Test failed / Reopened.
    Status,
    /// A step finished and the task waits for the user.
    Gate,
    /// A step finished; the next one is ready.
    StepDone,
    /// A step failed.
    Error,
    /// A quota window passed a warning level.
    Quota,
}

/// One line in the bell, also shown as a Windows notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertView {
    pub id: u64,
    pub at: String,
    pub kind: AlertKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wp_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub title: String,
    pub body: String,
    /// Task window tab that shows it (`comments`, `plan`, `findings`, `push`, `log`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    pub read: bool,
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
    /// Model and effort of the step's latest run (effort `None` = the CLI's default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// Why the user accepted it (or left it for later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The user asked for this one to be fixed (counts even when it is only low).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
    /// Cross-review round that reported it last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u8>,
    /// Step that reported it last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<StepKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateView {
    pub kind: GateKind,
    pub since: String,
    pub message: String,
    /// Findings and after-fix gates: the review whose findings they are about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<StepKind>,
    /// Where "go on" leads: past the findings gate without fixing, or "No" after a fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<StepKind>,
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

// ---------- workflow (steps 2-8) ----------

/// Where a task's workflow stands, as the task window and the card buttons need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlowStatus {
    /// Intake not done yet (should not happen for a started task).
    NotStarted,
    /// `current` is the next step; it runs when the user presses Run (or by itself when
    /// auto-advance is on).
    Ready,
    /// An AI process is working on `current` right now.
    Running,
    /// Waiting for the user: see `gate`.
    Gate,
    /// push-summary is ready; the user reviews, commits and pushes by hand.
    AwaitingPush,
    /// The user said the push is done.
    Done,
}

/// Why the last step failed, and whether signing in may help.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowError {
    /// `agent.missing`, `agent.signedOut`, `step.failed`, `step.stopped`, `step.timeout`,
    /// `step.budget`, `git.unexpected`, `findings.parse`, `repo.busy`, `repo.branch`, ...
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
}

/// The AI process running a step right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub step: StepKind,
    pub agent: AgentKind,
    pub started_at: String,
    /// Newest last.
    pub tools: Vec<ToolEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Model passed to the CLI, or the one it reported; `None` until known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// What runs: `/review-local` of the repo or the app's own prompt (reviews, push summary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// What a review said about documentation and tests (the `/review-local` docs/test section).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewNote {
    pub step: StepKind,
    pub text: String,
}

/// One fenced block of the push summary (commit message, PR description, ...), copied as is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushBlock {
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushView {
    /// Issue number passed to `/push-summary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<u64>,
    /// Whole answer, markdown.
    pub text: String,
    pub blocks: Vec<PushBlock>,
}

/// How one step run ended (the Tiến trình tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Running,
    Done,
    Failed,
}

/// One run of a step, for the Tiến trình tab (oldest first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: i64,
    pub step: StepKind,
    pub agent: AgentKind,
    pub state: RunState,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Cross-review round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u8>,
    /// What the run produced in one line ("báo 3 lỗi (1 high, 2 medium)"), or why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Tool calls the CLI refused (not on the allow list).
    #[serde(default)]
    pub denials: Vec<DeniedCall>,
    /// The raw output is still on disk, so the Chat tab can show it.
    pub has_chat: bool,
    /// Command that continues this run's session in a terminal (`claude --resume <id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
}

/// A tool call the CLI refused, with the allow-list entries that would let it run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeniedCall {
    /// `Bash`, `Edit`, …
    pub tool: String,
    /// The whole command (or the path).
    pub input: String,
    /// Several commands joined by `&&`, `;` or `|`: every part has to be allowed.
    pub chained: bool,
    /// `npx tsc *`, … (empty when the call is not a shell command).
    pub rules: Vec<RuleOption>,
}

/// One allow-list entry for `repos.<repo>.allowBash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleOption {
    pub rule: String,
    /// Already in the repo's allow list.
    pub allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatKind {
    /// What the app sent.
    Prompt,
    /// A message the AI wrote (markdown).
    Text,
    Thinking,
    Tool,
    /// The run ended; `tool` holds the CLI's result kind (`success`, `error_max_turns`, …).
    End,
    Error,
}

/// One item of the Chat tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatItem {
    pub kind: ChatKind,
    /// The prompt, the message, the tool's input (command, path) or the error.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// What the tool returned (cut).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Characters of the tool result left out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub output_cut: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    /// Why the CLI refused the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The Chat tab: the transcript of one step run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatView {
    pub run: RunSummary,
    pub items: Vec<ChatItem>,
    /// Older items left out (very long runs).
    #[serde(default)]
    pub dropped: u32,
    /// The raw output is gone (logs are kept 14 days).
    #[serde(default)]
    pub missing: bool,
}

/// One line of the task's log tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<StepKind>,
    /// `info`, `tool`, `ok`, `error`
    pub kind: String,
    pub text: String,
}

/// Everything the task window shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    pub wp_id: u64,
    pub task_id: String,
    pub title: String,
    pub project: String,
    /// The task is back in the Inbox: its history can be read, steps wait for "Bắt đầu".
    #[serde(default)]
    pub parked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub status: FlowStatus,
    /// Step that runs next, runs now, failed, or waits for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<StepKind>,
    pub steps: Vec<StepView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<FlowError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<RunView>,
    /// Cross-review rounds so far (no cap).
    pub round: u8,
    /// "Tự chạy": run the next step by itself after one finishes, stopping at every decision or
    /// error (off: stop after each step).
    pub auto_advance: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub findings: Vec<FindingView>,
    /// Docs/test judgement of the latest review of each kind (Tự review, Review chốt, …).
    #[serde(default)]
    pub review_notes: Vec<ReviewNote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<PushView>,
    /// Newest last.
    pub log: Vec<LogLine>,
    /// Every run of every step, oldest first (the Tiến trình tab).
    #[serde(default)]
    pub runs: Vec<RunSummary>,
    pub cost_usd: f64,
    /// Comments the user has not seen in the Comment tab (OpenProject + last issue read).
    #[serde(default)]
    pub new_comments: u32,
}

/// A button in the task window or on the card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FlowAction {
    /// Run the current step (the one shown as ready).
    Run,
    /// Stop the running step; it can be retried.
    Stop,
    /// Run the failed step again.
    Retry,
    /// Plan gate: the AI codes the plan.
    ApprovePlan,
    /// Plan gate: analyze again with this feedback.
    RejectPlan { feedback: String },
    /// Plan gate: the user codes by hand.
    SelfCode,
    /// The user finished coding by hand.
    SelfCodeDone,
    /// Findings gate: fix the picked findings (`fix`), or go on without fixing.
    PickFindings { fix: bool },
    /// After-fix gate: the answer to "Review chéo lại?".
    AfterFix { choice: AfterFix },
    /// Write the push summary again (optionally for another issue number).
    RerunPush {
        #[serde(default)]
        issue: Option<u64>,
    },
    /// The user pushed: the task moves to "Đã xong". Also allowed before the push summary (the
    /// remaining steps are skipped), never while a step runs.
    Finish {
        /// The push check said "not pushed" and the user marks it done anyway.
        #[serde(default)]
        forced: bool,
    },
    /// "Làm tiếp" on a finished task (or one waiting for the push): `step` = run again from
    /// there (`None` = carry on where it stopped); `note` goes to the next Viết code.
    Reopen {
        #[serde(default)]
        step: Option<StepKind>,
        #[serde(default)]
        note: Option<String>,
    },
    /// Turn auto-advance on/off for this task.
    SetAuto { on: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Decision {
    /// Fix it in the next fix step.
    Fix,
    /// Keep the code; `reason` says why.
    Accept,
    /// Leave it for later.
    Defer,
    /// Back to undecided.
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingDecision {
    pub wp_id: u64,
    pub fingerprint: String,
    pub decision: Decision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub task_id: String,
    /// OpenProject work package id (opens the task there).
    pub wp_id: u64,
    pub title: String,
    pub project: String,
    pub nx_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nx_status_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<PriorityView>,
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
            gate: None,
            flow_note: None,
            login: None,
            issues: None,
            finished_at: None,
            push_check: None,
            nx_open: false,
            priority: None,
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
        assert_eq!(s(serde_json::to_value(GateKind::Findings).unwrap()), "findings");
        assert_eq!(s(serde_json::to_value(GateKind::AfterFix).unwrap()), "afterFix");
        // saved before every review had a findings gate
        assert_eq!(
            serde_json::from_value::<GateKind>(json!("mediumTriage")).unwrap(),
            GateKind::Findings
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
        assert_eq!(s(serde_json::to_value(GateKind::SelfCode).unwrap()), "selfCode");
        assert_eq!(
            s(serde_json::to_value(FlowStatus::AwaitingPush).unwrap()),
            "awaitingPush"
        );
        assert_eq!(
            s(serde_json::to_value(SessionStatus::Waiting).unwrap()),
            "waiting"
        );
    }

    #[test]
    fn flow_actions_match_ts() {
        let a: FlowAction = serde_json::from_value(json!({"type": "rejectPlan", "feedback": "x"})).unwrap();
        assert_eq!(a, FlowAction::RejectPlan { feedback: "x".into() });
        let a: FlowAction = serde_json::from_value(json!({"type": "rerunPush"})).unwrap();
        assert_eq!(a, FlowAction::RerunPush { issue: None });
        let a: FlowAction = serde_json::from_value(json!({"type": "setAuto", "on": true})).unwrap();
        assert_eq!(a, FlowAction::SetAuto { on: true });
        let a: FlowAction = serde_json::from_value(json!({"type": "pickFindings", "fix": true})).unwrap();
        assert_eq!(a, FlowAction::PickFindings { fix: true });
        let a: FlowAction =
            serde_json::from_value(json!({"type": "afterFix", "choice": "crossReview"})).unwrap();
        assert_eq!(
            a,
            FlowAction::AfterFix {
                choice: AfterFix::CrossReview
            }
        );
        // the old button sends no `forced`; "Làm tiếp" may leave out the step and the note
        let a: FlowAction = serde_json::from_value(json!({"type": "finish"})).unwrap();
        assert_eq!(a, FlowAction::Finish { forced: false });
        let a: FlowAction = serde_json::from_value(json!({"type": "reopen"})).unwrap();
        assert_eq!(
            a,
            FlowAction::Reopen {
                step: None,
                note: None
            }
        );
        let a: FlowAction =
            serde_json::from_value(json!({"type": "reopen", "step": "implement", "note": "thêm test"}))
                .unwrap();
        assert_eq!(
            a,
            FlowAction::Reopen {
                step: Some(StepKind::Implement),
                note: Some("thêm test".into())
            }
        );
        let d: FindingDecision = serde_json::from_value(json!({
            "wpId": 1, "fingerprint": "ab", "decision": "accept", "reason": "ok"
        }))
        .unwrap();
        assert_eq!(d.decision, Decision::Accept);
        let c: StepChoiceRequest = serde_json::from_value(json!({
            "step": "finalReview", "model": "claude-sonnet-5-5"
        }))
        .unwrap();
        assert_eq!(
            (c.step, c.agent, c.model.as_deref(), c.effort),
            (StepKind::FinalReview, None, Some("claude-sonnet-5-5"), None)
        );
        let status = IssueStatus {
            at: "2026-11-14T10:40:00Z".into(),
            read: 2,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            json!({"at": "2026-11-14T10:40:00Z", "read": 2})
        );
    }
}
