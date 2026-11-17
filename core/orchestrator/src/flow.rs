//! Steps 2-8 of the workflow (PLAN.md §4). Each step runs as one headless AI process in the
//! task's repo; what it produces (plan, findings, push summary) lands in the task folder and
//! SQLite, and the state machine in `opw_domain::workflow` decides what comes next.
//!
//! Safety (PLAN.md §10):
//! - every AI process gets the git guard env and, for Claude, `DISALLOWED_GIT`;
//! - HEAD, the current branch and the local refs are compared before and after each step, and
//!   read-only steps (analyze, reviews, push summary) must also leave the working tree as it
//!   was; anything unexpected stops the flow with an error the user has to look at;
//! - by default the flow stops after each step (`steps.autoAdvance`), so the user sees every
//!   result before the next step starts; with "Tự chạy" on it still stops at every decision
//!   (plan, findings to fix, "Review chéo lại?") and every error, and says so (bell + toast);
//! - the user picks every finding the AI fixes; nothing is fixed by severity alone.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opw_agents::ModelChoice;
use opw_agents::claude::{self, ClaudeEvent, Denial, RunResult};
use opw_agents::codex::{self, CodexEvent};
use opw_agents::env::Account;
use opw_agents::run::{RunOutcome, RunSpec};
use opw_agents::text::{self, RawFinding};
use opw_agents::transcript;
use opw_config::{Config, RepoConfig, StepConfig, StepsConfig};
use opw_context::prompts::{self, FixItem};
use opw_domain::finding::{Severity, fingerprint};
use opw_domain::view::{
    AgentKind, AgentView, AlertKind, ApiError, CarryOn, ChatItem, ChatKind, ChatView, Decision,
    FindingDecision, FindingView, FlowAction, FlowError, FlowStatus, GateKind, GateView, LogLine, Provider,
    PushBlock, PushView, QuotaSnapshot, ReviewNote, RunState, RunSummary, RunView, SessionStatus, StepRef,
    StepView, TaskColumn, TaskDetail, ToolEvent,
};
use opw_domain::workflow::{AfterFix, Phase, StepKind, StepState, Workflow, WorkflowEvent};
use opw_storage::{FindingRow, StepRunEnd, StepRunRow, Store, TaskRow};
use opw_vcs::Git;
use serde::{Deserialize, Serialize};

pub const PLAN_FILE: &str = "plan.md";
pub const PUSH_FILE: &str = "push.md";
pub const BRIEF_FILE: &str = "brief.md";
const REVIEW_DIR: &str = "review";
/// Lines kept for the log tab.
const LOG_LINES: usize = 300;
/// Log lines read back from SQLite when the app starts.
const LOG_SEED: usize = 60;
/// Tool calls shown under the running step.
const TOOLS_SHOWN: usize = 8;
const CLAUDE_WINDOW: u64 = 1_000_000;
const CODEX_WINDOW: u64 = 500_000;
/// Codex hands off at about 80% of its window (PLAN.md §6).
const CODEX_HANDOFF: u64 = 400_000;
/// Error text shown on a card or in the log, at most this long.
const NOTE_CHARS: usize = 400;
/// Run logs older than this are deleted when the app starts.
const RUN_LOG_DAYS: u64 = 14;

// ---------- launching AI processes ----------

/// One AI process to run.
#[derive(Debug, Clone)]
pub struct Job {
    pub agent: Provider,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: Option<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The CLI is not installed (not on PATH).
    Missing,
    Spawn(String),
}

/// Starts AI processes. The real one runs the CLIs; tests replay fixtures.
pub trait Launcher: Send + Sync {
    fn launch(
        &self,
        job: &Job,
        stop: &AtomicBool,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<RunOutcome, LaunchError>;
}

/// Runs the installed `claude` / `codex` with the git guard (subscription login, API keys from
/// the environment are dropped).
#[derive(Debug, Clone)]
pub struct CliLauncher {
    hooks_dir: PathBuf,
}

impl CliLauncher {
    /// Writes the refusing git hooks into `hooks_dir`.
    pub fn new(hooks_dir: PathBuf) -> std::io::Result<Self> {
        opw_vcs::install_guard_hooks(&hooks_dir)?;
        Ok(Self { hooks_dir })
    }
}

impl Launcher for CliLauncher {
    fn launch(
        &self,
        job: &Job,
        stop: &AtomicBool,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<RunOutcome, LaunchError> {
        let program = match job.agent {
            Provider::Claude => opw_agents::cli::find_claude(),
            Provider::Codex => opw_agents::cli::find_codex(),
        }
        .ok_or(LaunchError::Missing)?;
        let spec = RunSpec {
            program,
            args: job.args.clone(),
            cwd: job.cwd.clone(),
            stdin: job.stdin.clone(),
            env: opw_vcs::guard_env(&self.hooks_dir),
            account: Account::Subscription,
            timeout: job.timeout,
        };
        opw_agents::run::run(&spec, stop, on_line).map_err(|e| LaunchError::Spawn(e.to_string()))
    }
}

// ---------- state ----------

/// What survives a restart (JSON in the `flow` table).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    workflow: Workflow,
    /// Claude session S1: analyze, then implement and fix resume it.
    session: Option<String>,
    /// Issue number for the push summary.
    issue: Option<u64>,
    error: Option<FlowError>,
    /// Per-task auto-advance; `None` = `steps.autoAdvance`.
    auto: Option<bool>,
    /// Sửa lỗi runs so far (no cap: the user decides after each one).
    fixes: u32,
    /// Feedback on the plan for the next analyze.
    feedback: Option<String>,
    /// When the phase last changed.
    since: Option<String>,
    /// A process was running when this was saved.
    in_flight: bool,
    /// The user said the push is done.
    finished: bool,
    /// What the user wrote with "Làm tiếp → quay lại Viết code"; goes into the next Viết code.
    note: Option<String>,
}

#[derive(Debug)]
struct Running {
    step: StepKind,
    agent: AgentKind,
    task_id: String,
    repo: Option<String>,
    started_at: String,
    stop: Arc<AtomicBool>,
    tools: VecDeque<ToolEvent>,
    context: Option<u64>,
    /// Passed to the CLI, or what Claude reported in its `init` line.
    model: Option<String>,
    effort: Option<String>,
    command: Option<String>,
    /// Row of this run in `step_run`, once the process started.
    run_id: Option<i64>,
}

#[derive(Debug, Default)]
struct Entry {
    saved: Saved,
    run: Option<Running>,
    log: VecDeque<LogLine>,
}

/// What a step run needs to know about its task.
#[derive(Debug, Clone)]
pub struct TaskCtx {
    pub row: TaskRow,
    /// `<tasks dir>/<task id>`: brief.md, plan.md, push.md, review/.
    pub task_dir: PathBuf,
    pub config: Config,
}

impl TaskCtx {
    fn repo(&self) -> Option<PathBuf> {
        self.row.repo.as_deref().map(opw_config::expand_home)
    }

    fn repo_config(&self) -> RepoConfig {
        let Some(repo) = self.row.repo.as_deref() else {
            return RepoConfig::default();
        };
        self.config
            .repos
            .iter()
            .find(|(k, _)| opw_config::same_path(k, repo))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    fn file(&self, name: &str) -> PathBuf {
        self.task_dir.join(name)
    }

    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.file(name)).ok()
    }

    fn auto(&self, saved: &Saved) -> bool {
        saved.auto.unwrap_or(self.config.steps.auto_advance)
    }

    fn agent_for(&self, step: StepKind) -> AgentKind {
        agent_for(step, &self.config)
    }
}

pub fn agent_for(step: StepKind, config: &Config) -> AgentKind {
    match step {
        StepKind::CrossReview if config.steps.cross_review.agent.as_deref() == Some("claude") => {
            AgentKind::Claude
        }
        StepKind::CrossReview => AgentKind::Codex,
        _ => AgentKind::Claude,
    }
}

/// `analyze`, `reviewLocal`, … as in JSON and in SQLite.
pub fn step_key(step: StepKind) -> &'static str {
    match step {
        StepKind::Intake => "intake",
        StepKind::Analyze => "analyze",
        StepKind::Implement => "implement",
        StepKind::Checks => "checks",
        StepKind::Harness => "harness",
        StepKind::ReviewLocal => "reviewLocal",
        StepKind::Fix => "fix",
        StepKind::CrossReview => "crossReview",
        StepKind::FixCross => "fixCross",
        StepKind::FinalReview => "finalReview",
        StepKind::BrowserTest => "browserTest",
        StepKind::PushSummary => "pushSummary",
    }
}

fn step_from_key(key: &str) -> Option<StepKind> {
    StepKind::ALL.into_iter().find(|s| step_key(*s) == key)
}

/// Vietnamese name of a step, as the UI shows it.
pub fn step_label(step: StepKind) -> &'static str {
    match step {
        StepKind::Intake => "Nhận task",
        StepKind::Analyze => "Phân tích",
        StepKind::Implement => "Viết code",
        StepKind::Checks => "Kiểm tra",
        StepKind::Harness => "Harness",
        StepKind::ReviewLocal => "Tự review",
        StepKind::Fix | StepKind::FixCross => "Sửa lỗi",
        StepKind::CrossReview => "Review chéo",
        StepKind::FinalReview => "Review chốt",
        StepKind::BrowserTest => "Test trình duyệt",
        StepKind::PushSummary => "Push summary",
    }
}

/// The `steps.<step>` section of a step (intake has none).
pub fn step_config(step: StepKind, steps: &StepsConfig) -> Option<&StepConfig> {
    Some(match step {
        // Kiểm tra runs no AI; Harness and Test trình duyệt get their settings when they run
        StepKind::Intake | StepKind::Checks | StepKind::Harness | StepKind::BrowserTest => return None,
        StepKind::Analyze => &steps.analyze,
        StepKind::Implement => &steps.implement,
        StepKind::ReviewLocal => &steps.review_local,
        // both Sửa lỗi steps share one setting
        StepKind::Fix | StepKind::FixCross => &steps.fix,
        StepKind::CrossReview => &steps.cross_review,
        StepKind::FinalReview => &steps.final_review,
        StepKind::PushSummary => &steps.push_summary,
    })
}

/// Model and effort to pass for a step: only what the user picked in Settings. The one value
/// the app adds is the push summary's `--effort low`, which it always used.
pub fn model_choice(step: StepKind, config: &Config) -> ModelChoice {
    let mut choice = step_config(step, &config.steps)
        .map(|c| ModelChoice::new(c.model.as_deref(), c.effort.as_deref()))
        .unwrap_or_default();
    if let Some(effort) = default_effort(step, config) {
        choice.effort.get_or_insert_with(|| effort.to_owned());
    }
    choice
}

/// Effort the app passes when the user picked none (push summary on Claude only).
pub fn default_effort(step: StepKind, config: &Config) -> Option<&'static str> {
    (step == StepKind::PushSummary && agent_for(step, config) == AgentKind::Claude)
        .then_some(claude::PUSH_SUMMARY_EFFORT)
}

/// What a review or push-summary step runs, for the status line and the log.
fn command_label(step: StepKind, agent: AgentKind, repo: Option<&Path>) -> Option<String> {
    let has = |name: &str| repo.is_some_and(|r| r.join(".claude").join("commands").join(name).is_file());
    Some(match step {
        StepKind::ReviewLocal | StepKind::FinalReview if has("review-local.md") => {
            "/review-local của repo".into()
        }
        StepKind::ReviewLocal | StepKind::FinalReview => "prompt review của app".into(),
        StepKind::CrossReview if agent == AgentKind::Codex => "codex exec, chỉ đọc".into(),
        StepKind::CrossReview => "prompt review của app".into(),
        StepKind::PushSummary if has("push-summary.md") => "/push-summary của repo".into(),
        StepKind::PushSummary => "prompt push summary của app".into(),
        _ => return None,
    })
}

/// `claude-opus-5-5 · high` for the log; empty parts left out.
fn model_text(model: Option<&str>, effort: Option<&str>) -> String {
    [model, effort]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn agent_name(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
        AgentKind::Shell => "shell",
    }
}

fn provider_of(agent: AgentKind) -> Provider {
    match agent {
        AgentKind::Codex => Provider::Codex,
        _ => Provider::Claude,
    }
}

/// Budget (USD) and timeout of a step: config first, then the app's defaults.
fn limits(step: StepKind, config: &Config) -> (f64, Duration, &'static str) {
    let steps = &config.steps;
    let (cfg, budget, minutes, key): (&StepConfig, f64, u32, &str) = match step {
        StepKind::Analyze => (&steps.analyze, 1.5, 20, "analyze"),
        StepKind::Implement => (&steps.implement, 5.0, 60, "implement"),
        StepKind::ReviewLocal => (&steps.review_local, 3.0, 30, "reviewLocal"),
        StepKind::FinalReview => (&steps.final_review, 3.0, 30, "finalReview"),
        StepKind::Fix | StepKind::FixCross => (&steps.fix, 2.0, 45, "fix"),
        StepKind::CrossReview => (&steps.cross_review, 3.0, 30, "crossReview"),
        StepKind::PushSummary => (&steps.push_summary, 1.0, 10, "pushSummary"),
        StepKind::Intake => (&steps.analyze, 0.0, 1, "intake"),
        // not run yet (later parts): the app's defaults
        StepKind::Checks => return (0.0, Duration::from_secs(10 * 60), "checks"),
        StepKind::Harness => return (2.0, Duration::from_secs(30 * 60), "harness"),
        StepKind::BrowserTest => return (3.0, Duration::from_secs(30 * 60), "browserTest"),
    };
    // the final review used to share `reviewLocal`; its limits still apply when unset
    let shared = (step == StepKind::FinalReview).then_some(&steps.review_local);
    let budget = cfg
        .max_budget_usd
        .or_else(|| shared.and_then(|c| c.max_budget_usd))
        .filter(|b| *b > 0.0)
        .unwrap_or(budget);
    let minutes = cfg
        .timeout_min
        .or_else(|| shared.and_then(|c| c.timeout_min))
        .filter(|m| *m > 0)
        .unwrap_or(minutes);
    (budget, Duration::from_secs(u64::from(minutes) * 60), key)
}

fn now() -> String {
    opw_quota::now_rfc3339()
}

fn flow_error(code: &str, message: impl Into<String>) -> FlowError {
    FlowError {
        code: code.into(),
        message: message.into(),
        provider: None,
    }
}

fn short(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= NOTE_CHARS {
        t.to_owned()
    } else {
        let cut: String = t.chars().take(NOTE_CHARS).collect();
        format!("{cut}…")
    }
}

/// Words the CLIs print when nobody is signed in or the login expired.
fn looks_signed_out(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "not logged in",
        "please run /login",
        "/login",
        "invalid api key",
        "authentication_error",
        "unauthorized",
        "oauth token has expired",
        "token expired",
        "please log in",
        "codex login",
    ]
    .iter()
    .any(|w| t.contains(w))
}

fn signed_out(agent: AgentKind) -> FlowError {
    let (name, cmd) = match agent {
        AgentKind::Codex => ("Codex", "codex login"),
        _ => ("Claude", "claude auth login"),
    };
    FlowError {
        code: "agent.signedOut".into(),
        message: format!(
            "{name} chưa đăng nhập hoặc phiên đăng nhập đã hết hạn. Bấm Đăng nhập (mở `{cmd}`), xong thì bấm Thử lại."
        ),
        provider: Some(provider_of(agent)),
    }
}

// ---------- findings ----------

fn severity_of(s: &str) -> Severity {
    match s {
        "critical" => Severity::Critical,
        "high" => Severity::High,
        "low" => Severity::Low,
        _ => Severity::Medium,
    }
}

fn settled(state: &str) -> bool {
    matches!(state, "fixed" | "accepted" | "deferred")
}

/// Picked by the user and still open (one being fixed, or fixed and not reviewed again, is not
/// taken twice).
fn to_fix(f: &FindingRow) -> bool {
    f.chosen && f.state == "open"
}

fn finding_view(f: &FindingRow) -> FindingView {
    let state = serde_json::from_value(serde_json::Value::String(f.state.clone()))
        .unwrap_or(opw_domain::finding::FindingState::Open);
    FindingView {
        fingerprint: f.fingerprint.clone(),
        severity: severity_of(&f.severity),
        title: f.title.clone(),
        file: f.file.clone(),
        line: f.line,
        source: f.source.clone(),
        state,
        detail: f.detail.clone().filter(|d| !d.is_empty()),
        suggestion: f.suggestion.clone().filter(|d| !d.is_empty()),
        reason: f.reason.clone(),
        chosen: f.chosen,
        round: f.round,
        step: f.step.as_deref().and_then(step_from_key),
    }
}

/// Merge what a review reported into the stored findings. Reported again → open (unless the
/// user accepted or deferred it); being fixed and not reported again → fixed.
fn merge_findings(
    store: &Store,
    wp_id: u64,
    step: StepKind,
    round: u8,
    source: &str,
    raw: &[RawFinding],
    at: &str,
) -> opw_storage::StorageResult<()> {
    let existing: HashMap<String, FindingRow> = store
        .findings(wp_id)?
        .into_iter()
        .map(|f| (f.fingerprint.clone(), f))
        .collect();
    let mut seen = HashSet::new();
    for r in raw {
        let fp = fingerprint(&r.file, &r.title);
        if !seen.insert(fp.clone()) {
            continue;
        }
        let old = existing.get(&fp);
        let keep = old.filter(|o| matches!(o.state.as_str(), "accepted" | "deferred"));
        store.upsert_finding(&FindingRow {
            wp_id,
            fingerprint: fp,
            severity: r.severity.clone(),
            title: r.title.trim().to_owned(),
            file: r.file.trim().to_owned(),
            line: r.line,
            source: source.to_owned(),
            state: keep.map_or_else(|| "open".to_owned(), |o| o.state.clone()),
            reason: keep.and_then(|o| o.reason.clone()),
            detail: Some(r.detail.clone()).filter(|d| !d.is_empty()),
            suggestion: Some(r.suggestion.clone()).filter(|d| !d.is_empty()),
            step: Some(step_key(step).to_owned()),
            round: (round > 0).then_some(round),
            chosen: old.is_some_and(|o| o.chosen),
            seen_at: Some(at.to_owned()),
        })?;
    }
    for old in existing.values() {
        if old.state == "fixing" && !seen.contains(&old.fingerprint) {
            store.upsert_finding(&FindingRow {
                state: "fixed".into(),
                ..old.clone()
            })?;
        }
    }
    Ok(())
}

// ---------- git safety ----------

#[derive(Debug, PartialEq, Eq)]
struct GitSnap {
    head: String,
    branch: String,
    refs: String,
    tree: String,
}

fn git_snap(repo: &Path) -> Result<GitSnap, String> {
    let git = Git::new(repo);
    let refs = git
        .run(&[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/tags",
        ])
        .map_err(|e| e.to_string())?;
    let status = git
        .run(&["status", "--porcelain", "--untracked-files=all"])
        .map_err(|e| e.to_string())?;
    let diff = git.run(&["diff", "HEAD", "--no-ext-diff"]).unwrap_or_default();
    Ok(GitSnap {
        head: git.head().unwrap_or_default(),
        branch: git.current_branch().unwrap_or_default(),
        refs,
        tree: fingerprint(&status, &diff),
    })
}

fn read_only(step: StepKind) -> bool {
    !matches!(step, StepKind::Implement | StepKind::Fix | StepKind::FixCross)
}

/// What changed that should not have.
fn git_surprise(step: StepKind, before: &GitSnap, after: &GitSnap) -> Option<String> {
    if before.head != after.head {
        return Some("HEAD đã đổi (có commit, reset hoặc checkout)".into());
    }
    if before.branch != after.branch {
        return Some(format!(
            "repo chuyển từ nhánh {} sang {}",
            before.branch, after.branch
        ));
    }
    if before.refs != after.refs {
        return Some("danh sách nhánh/tag đã đổi".into());
    }
    if read_only(step) && before.tree != after.tree {
        return Some("bước chỉ đọc nhưng file trong working tree đã đổi (bạn hoặc AI vừa sửa file?)".into());
    }
    None
}

// ---------- the engine ----------

/// Where Claude's `rate_limit_event`s go (the quota gauges).
type QuotaSink = Arc<dyn Fn(QuotaSnapshot) + Send + Sync>;

/// A step ended and the flow stopped: what to tell the user (bell + Windows notification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowAlert {
    pub wp_id: u64,
    pub task_id: String,
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
    /// Task window tab that shows it.
    pub tab: &'static str,
}

type AlertSink = Arc<dyn Fn(FlowAlert) + Send + Sync>;

struct Inner {
    tasks: Mutex<HashMap<u64, Entry>>,
    store: Arc<Mutex<Store>>,
    launcher: Arc<dyn Launcher>,
    /// `<data dir>/runs`: raw output of each run, for debugging.
    runs_dir: Option<PathBuf>,
    quota_sink: Mutex<Option<QuotaSink>>,
    alert_sink: Mutex<Option<AlertSink>>,
}

/// The workflow engine shared by the Tauri commands.
#[derive(Clone)]
pub struct Flow {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Flow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flow").finish_non_exhaustive()
    }
}

/// Card fields for one task in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct CardFlow {
    pub column: TaskColumn,
    pub step: StepRef,
    pub agent: Option<AgentKind>,
    pub cost_usd: Option<f64>,
    pub context_pct: Option<f64>,
    pub gate: Option<GateKind>,
    pub note: Option<String>,
    /// A failed step that needs this provider signed in again.
    pub login: Option<Provider>,
    /// When the user marked it done ("Đã xong" column).
    pub finished_at: Option<String>,
}

/// Orbit fields for one task in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemFlow {
    pub steps: Vec<StepView>,
    pub agents: Vec<AgentView>,
    pub findings: Vec<FindingView>,
    pub gate: Option<GateView>,
}

struct Claim {
    step: StepKind,
    agent: AgentKind,
    stop: Arc<AtomicBool>,
    choice: ModelChoice,
}

/// What a step produced: the event for the state machine, one line about the result (the
/// Tiến trình tab and the log) and what it cost.
struct Done {
    event: WorkflowEvent,
    note: String,
    cost: Option<f64>,
}

impl Flow {
    /// Load every saved workflow. A step that was running when the app closed is marked failed
    /// (its process is gone) so the user can retry it.
    pub fn new(store: Arc<Mutex<Store>>, launcher: Arc<dyn Launcher>, runs_dir: Option<PathBuf>) -> Self {
        let at = now();
        let mut tasks = HashMap::new();
        if let Ok(s) = store.lock() {
            let _ = s.fail_unfinished_runs(&at);
            for (wp_id, json) in s.flows().unwrap_or_default() {
                let Ok(mut saved) = serde_json::from_str::<Saved>(&json) else {
                    continue;
                };
                if saved.in_flight {
                    saved.in_flight = false;
                    if let Phase::Running(step) = saved.workflow.phase() {
                        let _ = saved.workflow.apply(WorkflowEvent::StepFailed(step));
                        saved.error = Some(flow_error(
                            "step.interrupted",
                            "App đã tắt khi bước này đang chạy. Bấm Thử lại để chạy lại.",
                        ));
                        saved.since = Some(at.clone());
                    }
                    if let Ok(json) = serde_json::to_string(&saved) {
                        let _ = s.save_flow(wp_id, &json, &at);
                    }
                }
                let log = s
                    .events(wp_id, LOG_SEED)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|e| {
                        let kind = e.kind.strip_prefix("flow.")?;
                        Some(LogLine {
                            at: e.at,
                            step: None,
                            kind: kind.to_owned(),
                            text: e.detail.unwrap_or_default(),
                        })
                    })
                    .collect();
                tasks.insert(
                    wp_id,
                    Entry {
                        saved,
                        run: None,
                        log,
                    },
                );
            }
        }
        if let Some(dir) = &runs_dir {
            prune_run_logs(dir);
        }
        Self {
            inner: Arc::new(Inner {
                tasks: Mutex::new(tasks),
                store,
                launcher,
                runs_dir,
                quota_sink: Mutex::new(None),
                alert_sink: Mutex::new(None),
            }),
        }
    }

    /// Where "a step ended and the task waits" goes (the bell and Windows notifications).
    pub fn set_alert_sink(&self, sink: impl Fn(FlowAlert) + Send + Sync + 'static) {
        if let Ok(mut s) = self.inner.alert_sink.lock() {
            *s = Some(Arc::new(sink));
        }
    }

    /// Where `rate_limit_event`s of Claude runs go (the quota gauges).
    pub fn set_quota_sink(&self, sink: impl Fn(QuotaSnapshot) + Send + Sync + 'static) {
        if let Ok(mut s) = self.inner.quota_sink.lock() {
            *s = Some(Arc::new(sink));
        }
    }

    /// The task was just started (or started again): intake is done, analyze is next. A task
    /// that was finished before starts over; one in progress keeps its state.
    /// The task was started (again) from the Inbox. A new task gets its workflow; one paused
    /// mid-way keeps its place; one that finished a round carries on like "Làm tiếp" (`carry_on`;
    /// `None` = where it stopped). Nothing is wiped: steps, plan, findings and history stay.
    pub fn started(&self, ctx: &TaskCtx, carry_on: Option<&CarryOn>) -> Result<(), ApiError> {
        let finished = {
            let mut tasks = self.inner.lock_tasks();
            let entry = tasks.entry(ctx.row.wp_id).or_default();
            if entry.saved.workflow.phase() == Phase::NotStarted {
                entry.saved = new_saved();
                push_log(entry, None, "info", "Đã nhận task: tạo nhánh và brief xong");
                self.inner.persist(ctx.row.wp_id, entry);
                return Ok(());
            }
            entry.saved.finished
        };
        if !finished {
            self.inner.log(
                ctx.row.wp_id,
                None,
                "info",
                "Bắt đầu lại từ Inbox: giữ nguyên chỗ đang làm",
                true,
            );
            return Ok(());
        }
        let carry_on = carry_on.cloned().unwrap_or_default();
        self.act(
            ctx.clone(),
            FlowAction::Reopen {
                step: carry_on.step,
                note: carry_on.note,
            },
        )
    }

    /// Make sure a task in progress has a workflow (tasks started before steps 2-8 existed).
    pub fn ensure(&self, ctx: &TaskCtx) {
        let mut tasks = self.inner.lock_tasks();
        if !tasks.contains_key(&ctx.row.wp_id) {
            let entry = tasks.entry(ctx.row.wp_id).or_default();
            entry.saved = new_saved();
            self.inner.persist(ctx.row.wp_id, entry);
        }
    }

    pub fn is_finished(&self, wp_id: u64) -> bool {
        self.inner
            .lock_tasks()
            .get(&wp_id)
            .is_some_and(|e| e.saved.finished)
    }

    /// The step a task is running right now, if any.
    pub fn running_step(&self, wp_id: u64) -> Option<StepKind> {
        self.inner
            .lock_tasks()
            .get(&wp_id)
            .and_then(|e| e.run.as_ref())
            .map(|r| r.step)
    }

    /// Stop the running step of a task, if any (pausing the task, closing the app).
    pub fn stop(&self, wp_id: u64) {
        if let Some(run) = self.inner.lock_tasks().get(&wp_id).and_then(|e| e.run.as_ref()) {
            run.stop.store(true, Ordering::Relaxed);
        }
    }

    /// A button of the task window or the card.
    pub fn act(&self, ctx: TaskCtx, action: FlowAction) -> Result<(), ApiError> {
        self.ensure(&ctx);
        let wp_id = ctx.row.wp_id;
        let start = {
            let mut tasks = self.inner.lock_tasks();
            let entry = tasks.get_mut(&wp_id).ok_or_else(flow_missing)?;
            let start = apply_action(entry, &ctx, &action, &self.inner.store)?;
            if !matches!(action, FlowAction::Run | FlowAction::Retry) || start {
                self.inner.persist(wp_id, entry);
            }
            start
        };
        if start {
            let claim = self.inner.claim(&ctx)?;
            let inner = self.inner.clone();
            std::thread::Builder::new()
                .name(format!("opw-flow-{wp_id}"))
                .spawn(move || worker(&inner, &ctx, claim))
                .map_err(|e| ApiError::new("flow.thread", e.to_string()))?;
        }
        Ok(())
    }

    /// Fix / accept / defer / reset one finding.
    pub fn decide(&self, d: &FindingDecision) -> Result<(), ApiError> {
        let store = self.inner.lock_store()?;
        let mut f = store
            .findings(d.wp_id)
            .map_err(|e| ApiError::new("storage", e.to_string()))?
            .into_iter()
            .find(|f| f.fingerprint == d.fingerprint)
            .ok_or_else(|| ApiError::new("finding.notFound", "Không thấy lỗi này nữa"))?;
        let reason = d.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
        match d.decision {
            Decision::Fix => {
                f.chosen = true;
                if settled(&f.state) {
                    f.state = "open".into();
                }
                f.reason = None;
            }
            Decision::Accept => {
                let reason = reason.ok_or_else(|| {
                    ApiError::new(
                        "finding.reason",
                        "Chấp nhận thì ghi lý do (vì sao giữ code như vậy)",
                    )
                })?;
                f.state = "accepted".into();
                f.chosen = false;
                f.reason = Some(reason.to_owned());
            }
            Decision::Defer => {
                f.state = "deferred".into();
                f.chosen = false;
                f.reason = reason.map(str::to_owned);
            }
            Decision::Reset => {
                f.state = "open".into();
                f.chosen = false;
                f.reason = None;
            }
        }
        store
            .upsert_finding(&f)
            .map_err(|e| ApiError::new("storage", e.to_string()))
    }

    pub fn card(&self, ctx: &TaskCtx) -> Option<CardFlow> {
        let tasks = self.inner.lock_tasks();
        let entry = tasks.get(&ctx.row.wp_id)?;
        let runs = self.inner.runs(ctx.row.wp_id);
        let status = status_of(entry);
        let wf = &entry.saved.workflow;
        let kind = current_step(entry).unwrap_or(StepKind::Intake);
        let state = match status {
            FlowStatus::Running => StepState::Running,
            FlowStatus::Ready => StepState::Pending,
            FlowStatus::Gate if wf.phase() == Phase::Gate(GateKind::Error) => StepState::Failed,
            FlowStatus::Gate => StepState::WaitingUser,
            FlowStatus::AwaitingPush | FlowStatus::Done => StepState::Done,
            FlowStatus::NotStarted => StepState::Pending,
        };
        let gate = match wf.phase() {
            Phase::Gate(kind) => Some(kind),
            _ => None,
        };
        let note = match (status, &entry.saved.error) {
            (FlowStatus::Gate, Some(e)) if gate == Some(GateKind::Error) => Some(short(&e.message)),
            (FlowStatus::Gate, _) => {
                gate.map(|g| gate_message(g, entry, &self.inner.findings(ctx.row.wp_id)))
            }
            _ => None,
        };
        let cost: f64 = runs.iter().filter_map(|r| r.cost_usd).sum();
        Some(CardFlow {
            column: match status {
                FlowStatus::Done => TaskColumn::Done,
                FlowStatus::AwaitingPush => TaskColumn::AwaitingPush,
                _ => TaskColumn::Active,
            },
            finished_at: entry.saved.finished.then(|| entry.saved.since.clone()).flatten(),
            step: StepRef {
                kind,
                index: kind.index(),
                state,
                since: entry
                    .run
                    .as_ref()
                    .map(|r| r.started_at.clone())
                    .or_else(|| entry.saved.since.clone())
                    .unwrap_or_else(|| ctx.row.updated_at.clone()),
            },
            agent: entry.run.as_ref().map(|r| r.agent),
            cost_usd: (cost > 0.0).then_some(cost),
            context_pct: entry.run.as_ref().and_then(|r| {
                let window = if r.agent == AgentKind::Codex {
                    CODEX_WINDOW
                } else {
                    CLAUDE_WINDOW
                };
                r.context.map(|c| c as f64 * 100.0 / window as f64)
            }),
            login: (gate == Some(GateKind::Error))
                .then(|| entry.saved.error.as_ref().and_then(|e| e.provider))
                .flatten(),
            gate,
            note,
        })
    }

    pub fn system(&self, ctx: &TaskCtx) -> Option<SystemFlow> {
        let tasks = self.inner.lock_tasks();
        let entry = tasks.get(&ctx.row.wp_id)?;
        let runs = self.inner.runs(ctx.row.wp_id);
        let findings = self.inner.findings(ctx.row.wp_id);
        let status = status_of(entry);
        let handoff = ctx.config.handoff.claude_at_tokens;
        let agents = match (&entry.run, current_step(entry)) {
            (Some(run), _) => vec![agent_view(
                ctx.row.wp_id,
                run.agent,
                SessionStatus::Running,
                run.step,
                run.context,
                run.tools.iter().cloned().collect(),
                handoff,
            )],
            (None, Some(step)) if matches!(status, FlowStatus::Ready | FlowStatus::Gate) => {
                let s = if status == FlowStatus::Gate {
                    SessionStatus::Waiting
                } else {
                    SessionStatus::Idle
                };
                vec![agent_view(
                    ctx.row.wp_id,
                    ctx.agent_for(step),
                    s,
                    step,
                    None,
                    Vec::new(),
                    handoff,
                )]
            }
            _ => Vec::new(),
        };
        Some(SystemFlow {
            steps: step_views(entry, &runs, &ctx.row.updated_at),
            agents,
            findings: findings.iter().map(finding_view).collect(),
            gate: gate_view(entry, &findings),
        })
    }

    /// Everything the task window shows. Reads the task files; call it off the UI thread.
    pub fn detail(&self, ctx: &TaskCtx) -> TaskDetail {
        self.ensure(ctx);
        let runs = self.inner.runs(ctx.row.wp_id);
        let findings = self.inner.findings(ctx.row.wp_id);
        let tasks = self.inner.lock_tasks();
        let entry = tasks.get(&ctx.row.wp_id);
        let empty = Entry::default();
        let entry = entry.unwrap_or(&empty);
        let push = ctx
            .read(PUSH_FILE)
            .map(|text| push_view(&text, entry.saved.issue));
        TaskDetail {
            wp_id: ctx.row.wp_id,
            task_id: ctx.row.display_id.clone(),
            title: ctx.row.title.clone(),
            project: ctx.row.project.clone(),
            parked: ctx.row.column == "inbox",
            repo: ctx.row.repo.clone(),
            branch: ctx.row.branch.clone(),
            base: ctx.row.base.clone(),
            status: status_of(entry),
            current: current_step(entry),
            steps: step_views(entry, &runs, &ctx.row.updated_at),
            gate: gate_view(entry, &findings),
            error: entry.saved.error.clone(),
            running: entry.run.as_ref().map(|r| RunView {
                step: r.step,
                agent: r.agent,
                started_at: r.started_at.clone(),
                tools: r.tools.iter().cloned().collect(),
                context_tokens: r.context,
                model: r.model.clone(),
                effort: r.effort.clone(),
                command: r.command.clone(),
            }),
            round: entry.saved.workflow.round(),
            auto_advance: ctx.auto(&entry.saved),
            brief: ctx.read(BRIEF_FILE),
            plan: ctx.read(PLAN_FILE),
            findings: findings.iter().map(finding_view).collect(),
            review_notes: review_notes(ctx),
            push,
            log: entry.log.iter().cloned().collect(),
            runs: {
                let allowed = ctx.repo_config().allow_bash;
                runs.iter()
                    .filter_map(|r| {
                        run_summary(
                            r,
                            entry.run.as_ref(),
                            self.inner.runs_dir.as_deref(),
                            &ctx.row.display_id,
                            &allowed,
                        )
                    })
                    .collect()
            },
            cost_usd: runs.iter().filter_map(|r| r.cost_usd).sum(),
            // the facade fills this from OpenProject and the last issue read
            new_comments: 0,
        }
    }

    /// The Chat tab: the transcript of one run (`None` = the latest). Reads the run's raw log;
    /// call it off the UI thread.
    pub fn chat(&self, ctx: &TaskCtx, run_id: Option<i64>) -> Result<ChatView, ApiError> {
        let runs = self.inner.runs(ctx.row.wp_id);
        let row = match run_id {
            Some(id) => runs.iter().find(|r| r.id == id),
            None => runs.last(),
        }
        .ok_or_else(|| ApiError::new("run.missing", "Chưa có lần chạy nào"))?;
        let summary = {
            let tasks = self.inner.lock_tasks();
            let live = tasks.get(&ctx.row.wp_id).and_then(|e| e.run.as_ref());
            run_summary(
                row,
                live,
                self.inner.runs_dir.as_deref(),
                &ctx.row.display_id,
                &ctx.repo_config().allow_bash,
            )
        }
        .ok_or_else(|| ApiError::new("run.missing", "Không đọc được lần chạy này"))?;
        let text = log_path(row, self.inner.runs_dir.as_deref(), &ctx.row.display_id)
            .and_then(|p| std::fs::read_to_string(p).ok());
        let t = transcript::read(
            text.as_deref().unwrap_or_default(),
            summary.agent == AgentKind::Codex,
        );
        Ok(ChatView {
            run: summary,
            items: t.items.into_iter().map(chat_item).collect(),
            dropped: u32::try_from(t.dropped).unwrap_or(u32::MAX),
            missing: text.is_none(),
        })
    }

    /// Repo, CLI and session id to continue a finished run in a terminal.
    pub fn resume_target(&self, ctx: &TaskCtx, run_id: i64) -> Result<(PathBuf, Provider, String), ApiError> {
        let runs = self.inner.runs(ctx.row.wp_id);
        let row = runs
            .iter()
            .find(|r| r.id == run_id)
            .ok_or_else(|| ApiError::new("run.missing", "Không thấy lần chạy này"))?;
        if self
            .inner
            .lock_tasks()
            .get(&ctx.row.wp_id)
            .is_some_and(|e| e.run.is_some())
        {
            return Err(ApiError::new(
                "step.running",
                "Task đang chạy một bước. Chờ bước đó xong rồi mở tiếp trong terminal.",
            ));
        }
        let step = step_from_key(&row.step)
            .ok_or_else(|| ApiError::new("run.missing", "Không đọc được lần chạy này"))?;
        let agent = agent_of_row(row);
        let session = row
            .session_id
            .clone()
            .filter(|_| resume_command(step, agent, row.session_id.as_deref()).is_some())
            .ok_or_else(|| {
                ApiError::new(
                    "run.noSession",
                    "Lần chạy này không lưu phiên nên không mở tiếp được",
                )
            })?;
        let repo = ctx.repo().filter(|p| p.is_dir()).ok_or_else(|| {
            ApiError::new(
                "repo.missing",
                "Không thấy thư mục repo của task (đã xoá hoặc đổi chỗ?)",
            )
        })?;
        Ok((repo, provider_of(agent), session))
    }

    /// Cost and tokens of every step run since `since` (the bottom bar's "today").
    pub fn usage_since(&self, since: &str) -> (f64, u64) {
        self.inner
            .store
            .lock()
            .ok()
            .and_then(|s| s.usage_since(since).ok())
            .unwrap_or_default()
    }
}

fn new_saved() -> Saved {
    let mut workflow = Workflow::new();
    let _ = workflow.apply(WorkflowEvent::Start);
    let _ = workflow.apply(WorkflowEvent::StepSucceeded(StepKind::Intake));
    Saved {
        workflow,
        since: Some(now()),
        ..Saved::default()
    }
}

fn agent_of_row(row: &StepRunRow) -> AgentKind {
    if row.agent.as_deref() == Some("codex") {
        AgentKind::Codex
    } else {
        AgentKind::Claude
    }
}

/// `claude --resume <id>` / `codex resume <id>` for a run whose session the CLI kept. The push
/// summary runs with `--no-session-persistence`, so it has none. Only plain ids are used (the
/// command goes into a console window).
fn resume_command(step: StepKind, agent: AgentKind, session: Option<&str>) -> Option<String> {
    let id = session.filter(|s| {
        !s.is_empty() && s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })?;
    match agent {
        AgentKind::Codex => Some(format!("codex resume {id}")),
        _ if step == StepKind::PushSummary => None,
        _ => Some(format!("claude --resume {id}")),
    }
}

/// Refused calls as stored (`[{"tool":"Bash","input":"…"}]`).
fn denials_json(denials: &[Denial]) -> Option<String> {
    (!denials.is_empty()).then(|| {
        serde_json::Value::Array(
            denials
                .iter()
                .map(|d| serde_json::json!({ "tool": d.tool, "input": d.input }))
                .collect(),
        )
        .to_string()
    })
}

fn stored_denials(json: Option<&str>) -> Vec<(String, String)> {
    let Some(v) = json.and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok()) else {
        return Vec::new();
    };
    v.as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            let s = |k: &str| {
                d.get(k)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            (s("tool"), s("input"))
        })
        .collect()
}

/// One row of the Tiến trình tab. `live` is the run in progress, if any (its model is known
/// before the row has it).
/// The run's raw log, if it is still there. Runs from before migration 5 have no `log_file`:
/// their log is the file of the same step named after the moment it was created, right after
/// the run started (`<runs>/<task>/<2026-11-14T10-27-41-…>-<step>.jsonl`).
fn log_path(row: &StepRunRow, runs_dir: Option<&Path>, task: &str) -> Option<PathBuf> {
    let dir = runs_dir?;
    if let Some(file) = row.log_file.as_deref() {
        let p = dir.join(file);
        return p.is_file().then_some(p);
    }
    let folder = dir.join(task);
    let suffix = format!("-{}.jsonl", row.step);
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.is_ascii() && n.len() > 19 && n.ends_with(&suffix))
        .collect();
    names.sort();
    names
        .into_iter()
        .find(|n| {
            // "2026-11-14T10-27-41" → "2026-11-14T10:27:41Z"
            let stamp = format!("{}{}Z", &n[..11], n[11..19].replace('-', ":"));
            opw_quota::seconds_between(&row.started_at, &stamp).is_some_and(|s| (-1.0..=5.0).contains(&s))
        })
        .map(|n| folder.join(n))
}

fn run_summary(
    row: &StepRunRow,
    live: Option<&Running>,
    runs_dir: Option<&Path>,
    task: &str,
    allowed: &[String],
) -> Option<RunSummary> {
    let step = step_from_key(&row.step)?;
    let agent = agent_of_row(row);
    let live = live.filter(|r| r.run_id == Some(row.id));
    let state = match row.state.as_str() {
        "done" => RunState::Done,
        "running" => RunState::Running,
        _ => RunState::Failed,
    };
    Some(RunSummary {
        id: row.id,
        step,
        agent,
        state,
        started_at: row.started_at.clone(),
        ended_at: row.ended_at.clone(),
        cost_usd: row.cost_usd,
        tokens_in: row.tokens_in,
        tokens_out: row.tokens_out,
        model: live.and_then(|r| r.model.clone()).or_else(|| row.model.clone()),
        effort: live.and_then(|r| r.effort.clone()).or_else(|| row.effort.clone()),
        round: row.round,
        note: row.note.clone(),
        denials: stored_denials(row.denials.as_deref())
            .into_iter()
            .map(|(tool, input)| crate::allow::denied_call(&tool, &input, allowed))
            .collect(),
        has_chat: log_path(row, runs_dir, task).is_some(),
        resume: (state != RunState::Running)
            .then(|| resume_command(step, agent, row.session_id.as_deref()))
            .flatten(),
    })
}

fn chat_item(i: transcript::Item) -> ChatItem {
    use transcript::ItemKind as K;
    ChatItem {
        kind: match i.kind {
            K::Prompt => ChatKind::Prompt,
            K::Text => ChatKind::Text,
            K::Thinking => ChatKind::Thinking,
            K::Tool => ChatKind::Tool,
            K::End => ChatKind::End,
            K::Error => ChatKind::Error,
        },
        text: i.text,
        tool: i.tool,
        output: i.output,
        output_cut: u32::try_from(i.output_cut).unwrap_or(u32::MAX),
        failed: i.failed,
        denied: i.denied,
    }
}

/// What the app asked: the stdin prompt, or the `-p` command plus its added instructions.
fn prompt_of(job: &Job) -> String {
    if let Some(stdin) = &job.stdin {
        return stdin.clone();
    }
    let after = |flag: &str| job.args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone());
    let head = after("-p").filter(|a| !a.starts_with("--")).unwrap_or_default();
    match after("--append-system-prompt") {
        Some(extra) if !head.is_empty() => format!("{head}\n\n{extra}"),
        Some(extra) => extra,
        None => head,
    }
}

/// "plan: 3 file sẽ sửa, 2 câu hỏi" from the sections `prompts::analyze` asks for.
fn plan_note(plan: &str) -> String {
    let section = |title: &str| -> Vec<&str> {
        let mut lines = plan
            .lines()
            .skip_while(|l| !(l.trim_start().starts_with("## ") && l.contains(title)));
        lines.next();
        lines.take_while(|l| !l.trim_start().starts_with("## ")).collect()
    };
    // top-level list items only (sub-bullets are details of a file)
    let files = section("File sẽ sửa")
        .iter()
        .filter(|l| l.starts_with(['-', '*', '+']) || l.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .count();
    let questions = section("câu hỏi")
        .iter()
        .filter(|l| l.contains('?') || l.contains('？'))
        .count();
    let mut parts = Vec::new();
    if files > 0 {
        parts.push(format!("{files} file sẽ sửa"));
    }
    if questions > 0 {
        parts.push(format!("{questions} câu hỏi"));
    }
    if parts.is_empty() {
        "đã viết plan.md".into()
    } else {
        format!("plan: {}", parts.join(", "))
    }
}

/// "4 file thay đổi (+120 −8)": the whole working tree against HEAD (the AI never commits).
fn change_note(repo: &Path) -> String {
    let git = Git::new(repo);
    let (mut files, mut added, mut removed) = (0_usize, 0_u64, 0_u64);
    for line in git
        .run(&["diff", "HEAD", "--numstat", "--no-ext-diff"])
        .unwrap_or_default()
        .lines()
    {
        let mut cols = line.split('\t');
        let (Some(a), Some(r)) = (cols.next(), cols.next()) else {
            continue;
        };
        files += 1;
        added += a.parse::<u64>().unwrap_or(0);
        removed += r.parse::<u64>().unwrap_or(0);
    }
    files += git
        .run(&["ls-files", "--others", "--exclude-standard"])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    if files == 0 {
        "không có file nào thay đổi".into()
    } else {
        format!("{files} file thay đổi (+{added} −{removed})")
    }
}

/// ", 1 lệnh bị chặn" when the CLI refused tool calls.
fn denied_note(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        format!(", {n} lệnh bị chặn")
    }
}

fn flow_missing() -> ApiError {
    ApiError::new(
        "flow.missing",
        "Task này chưa bắt đầu (bấm Bắt đầu làm việc trước)",
    )
}

fn not_now(what: &str) -> ApiError {
    ApiError::new("flow.notNow", format!("Lúc này chưa {what} được"))
}

/// Apply a button to the saved state. Returns `true` when a step should start now.
fn apply_action(
    entry: &mut Entry,
    ctx: &TaskCtx,
    action: &FlowAction,
    store: &Mutex<Store>,
) -> Result<bool, ApiError> {
    use WorkflowEvent as E;
    let running = entry.run.is_some();
    let phase = entry.saved.workflow.phase();
    let event = |entry: &mut Entry, e: WorkflowEvent| -> Result<(), ApiError> {
        entry
            .saved
            .workflow
            .apply(e)
            .map_err(|e| ApiError::new("flow.invalid", e.to_string()))?;
        entry.saved.since = Some(now());
        Ok(())
    };
    match action {
        FlowAction::Run => {
            if running {
                return Err(ApiError::new("step.running", "Bước này đang chạy"));
            }
            match phase {
                Phase::Running(_) => Ok(true),
                Phase::Gate(GateKind::Error) => {
                    let step = entry
                        .saved
                        .workflow
                        .failed_step()
                        .ok_or_else(|| not_now("chạy"))?;
                    event(entry, E::Retry(step))?;
                    entry.saved.error = None;
                    Ok(true)
                }
                _ => Err(not_now("chạy bước tiếp")),
            }
        }
        FlowAction::Retry => {
            let step = entry
                .saved
                .workflow
                .failed_step()
                .ok_or_else(|| not_now("thử lại"))?;
            event(entry, E::Retry(step))?;
            entry.saved.error = None;
            push_log(
                entry,
                Some(step),
                "info",
                &format!("Thử lại {}", step_label(step)),
            );
            Ok(true)
        }
        FlowAction::Stop => {
            let run = entry
                .run
                .as_ref()
                .ok_or_else(|| not_now("dừng (không có bước nào đang chạy)"))?;
            run.stop.store(true, Ordering::Relaxed);
            Ok(false)
        }
        FlowAction::ApprovePlan => {
            if ctx.read(PLAN_FILE).is_none_or(|p| p.trim().is_empty()) {
                return Err(ApiError::new("plan.missing", "Chưa có plan.md để duyệt"));
            }
            event(entry, E::PlanApproved)?;
            push_log(
                entry,
                Some(StepKind::Analyze),
                "ok",
                "Bạn duyệt plan: AI viết code",
            );
            Ok(true)
        }
        FlowAction::RejectPlan { feedback } => {
            let feedback = feedback.trim();
            if feedback.is_empty() {
                return Err(ApiError::new("plan.feedback", "Ghi góp ý cho plan trước"));
            }
            event(entry, E::PlanRejected)?;
            entry.saved.feedback = Some(feedback.to_owned());
            push_log(
                entry,
                Some(StepKind::Analyze),
                "info",
                "Bạn góp ý plan: phân tích lại",
            );
            Ok(true)
        }
        FlowAction::SelfCode => {
            event(entry, E::SelfCode)?;
            push_log(
                entry,
                Some(StepKind::Implement),
                "info",
                "Bạn tự code; xong thì bấm Tôi code xong",
            );
            Ok(false)
        }
        FlowAction::SelfCodeDone => {
            event(entry, E::SelfCodeDone)?;
            push_log(
                entry,
                Some(StepKind::Implement),
                "ok",
                "Bạn code xong: chạy review",
            );
            Ok(true)
        }
        FlowAction::PickFindings { fix } => {
            if phase != Phase::Gate(GateKind::Findings) {
                return Err(not_now("chọn lỗi để sửa"));
            }
            let review = entry
                .saved
                .workflow
                .gate_review()
                .unwrap_or(StepKind::FinalReview);
            let picked = if *fix {
                let findings = store
                    .lock()
                    .ok()
                    .and_then(|s| s.findings(ctx.row.wp_id).ok())
                    .unwrap_or_default();
                let n = findings.iter().filter(|f| to_fix(f)).count();
                if n == 0 {
                    return Err(ApiError::new(
                        "findings.none",
                        "Chưa chọn lỗi nào: bấm Sửa ở lỗi cần AI sửa trước",
                    ));
                }
                n
            } else {
                0
            };
            event(entry, E::FindingsPicked { fix: picked })?;
            let text = if picked > 0 {
                format!("Bạn chọn {picked} lỗi của {} cho AI sửa", step_label(review))
            } else {
                format!(
                    "Bạn không sửa lỗi nào của {}: sang {}",
                    step_label(review),
                    current_step(entry).map_or("bước sau", step_label)
                )
            };
            push_log(entry, Some(review), "info", &text);
            Ok(matches!(entry.saved.workflow.phase(), Phase::Running(_)))
        }
        FlowAction::AfterFix { choice } => {
            if phase != Phase::Gate(GateKind::AfterFix) {
                return Err(not_now("trả lời câu này"));
            }
            let fix = current_step(entry);
            event(entry, E::AfterFix(*choice))?;
            let wf = &entry.saved.workflow;
            let text = match choice {
                AfterFix::CrossReview => format!("Bạn chọn Review chéo lại (vòng {})", wf.round()),
                AfterFix::PickMore => "Bạn chọn thêm lỗi để sửa".to_owned(),
                AfterFix::Next => format!(
                    "Không review chéo lại: sang {}",
                    wf.current_step().map_or("bước sau", step_label)
                ),
            };
            push_log(entry, fix, "info", &text);
            Ok(matches!(entry.saved.workflow.phase(), Phase::Running(_)))
        }
        FlowAction::RerunPush { issue } => {
            if running {
                return Err(ApiError::new("step.running", "Bước này đang chạy"));
            }
            if issue.is_some() {
                entry.saved.issue = *issue;
            }
            if phase == Phase::AwaitingPush {
                event(entry, E::RedoPushSummary)?;
            } else if phase != Phase::Running(StepKind::PushSummary) {
                return Err(not_now("viết lại push summary"));
            }
            Ok(true)
        }
        FlowAction::Finish { forced } => {
            if running {
                return Err(ApiError::new(
                    "step.running",
                    "Bước này đang chạy. Bấm Dừng trước rồi mới đánh dấu xong.",
                ));
            }
            if phase == Phase::NotStarted || entry.saved.finished {
                return Err(not_now("đánh dấu xong"));
            }
            entry.saved.finished = true;
            entry.saved.since = Some(now());
            let text = match (phase == Phase::AwaitingPush, *forced) {
                (true, false) => "Bạn đã push: task xong".to_owned(),
                (true, true) => "Bạn đánh dấu xong khi app chưa thấy nhánh trên origin".to_owned(),
                (false, _) => format!(
                    "Bạn đánh dấu xong ở bước {} (bỏ qua các bước còn lại)",
                    entry.saved.workflow.current_step().map_or("?", step_label)
                ),
            };
            push_log(entry, None, "ok", &text);
            Ok(false)
        }
        FlowAction::Reopen { step, note } => {
            if running {
                return Err(ApiError::new("step.running", "Bước này đang chạy"));
            }
            if phase == Phase::NotStarted {
                return Err(not_now("làm tiếp"));
            }
            if let Some(step) = *step {
                event(entry, E::Reopen(step))?;
                // the Sửa lỗi count starts over with the review loop
                if step.index() <= StepKind::CrossReview.index() {
                    entry.saved.fixes = 0;
                }
                if step == StepKind::Analyze {
                    entry.saved.feedback = None;
                }
            }
            entry.saved.finished = false;
            entry.saved.error = None;
            entry.saved.since = Some(now());
            let note = note.as_deref().map(str::trim).filter(|n| !n.is_empty());
            if let Some(note) = note {
                entry.saved.note = Some(note.to_owned());
            }
            let text = match *step {
                Some(step) => format!("Làm tiếp: chạy lại từ {}", step_label(step)),
                None => format!(
                    "Làm tiếp ở {}",
                    current_step(entry).map_or("bước đang dở", step_label)
                ),
            };
            let text = match note {
                Some(note) => format!("{text}, kèm ghi chú: {}", short(note)),
                None => text,
            };
            push_log(entry, *step, "info", &text);
            Ok(false)
        }
        FlowAction::SetAuto { on } => {
            entry.saved.auto = Some(*on);
            Ok(false)
        }
    }
}

fn push_log(entry: &mut Entry, step: Option<StepKind>, kind: &str, text: &str) {
    entry.log.push_back(LogLine {
        at: now(),
        step,
        kind: kind.to_owned(),
        text: text.to_owned(),
    });
    while entry.log.len() > LOG_LINES {
        entry.log.pop_front();
    }
}

/// What to announce when the flow stops after `step`. Nothing when the user pressed Dừng.
fn stop_alert(ctx: &TaskCtx, step: StepKind, entry: &Entry, text: &str) -> Option<FlowAlert> {
    if entry
        .saved
        .error
        .as_ref()
        .is_some_and(|e| e.code == "step.stopped")
    {
        return None;
    }
    let task_id = ctx.row.display_id.clone();
    let title_of = |what: &str| format!("{task_id} · {what}");
    let (kind, title, body, tab) = match entry.saved.workflow.phase() {
        Phase::Gate(GateKind::Error) => (
            AlertKind::Error,
            title_of(&format!("{} lỗi", step_label(step))),
            entry
                .saved
                .error
                .as_ref()
                .map_or_else(|| text.to_owned(), |e| e.message.clone()),
            "progress",
        ),
        Phase::Gate(GateKind::PlanReview) => (
            AlertKind::Gate,
            title_of("plan chờ bạn duyệt"),
            ctx.row.title.clone(),
            "plan",
        ),
        Phase::Gate(GateKind::Findings) => (
            AlertKind::Gate,
            title_of(&format!(
                "{} báo lỗi: chọn lỗi cần sửa",
                step_label(entry.saved.workflow.gate_review().unwrap_or(step))
            )),
            ctx.row.title.clone(),
            "findings",
        ),
        Phase::Gate(GateKind::AfterFix) => (
            AlertKind::Gate,
            title_of("sửa lỗi xong: Review chéo lại?"),
            ctx.row.title.clone(),
            "progress",
        ),
        Phase::Gate(_) => (
            AlertKind::Gate,
            title_of("chờ bạn"),
            ctx.row.title.clone(),
            "brief",
        ),
        Phase::AwaitingPush => (
            AlertKind::Gate,
            title_of("sẵn sàng push"),
            "Commit message và PR description đã có: copy rồi tự push.".to_owned(),
            "push",
        ),
        Phase::Running(_) | Phase::Paused => {
            let next = entry.saved.workflow.current_step()?;
            (
                AlertKind::StepDone,
                title_of(&format!("{} xong", step_label(step))),
                format!("Bước tiếp: {}. Bấm Chạy khi bạn sẵn sàng.", step_label(next)),
                "progress",
            )
        }
        _ => return None,
    };
    Some(FlowAlert {
        wp_id: ctx.row.wp_id,
        task_id,
        kind,
        title,
        body,
        tab,
    })
}

fn status_of(entry: &Entry) -> FlowStatus {
    if entry.saved.finished {
        return FlowStatus::Done;
    }
    if entry.run.is_some() {
        return FlowStatus::Running;
    }
    match entry.saved.workflow.phase() {
        Phase::NotStarted => FlowStatus::NotStarted,
        Phase::Running(_) | Phase::Paused => FlowStatus::Ready,
        Phase::Gate(_) => FlowStatus::Gate,
        Phase::AwaitingPush => FlowStatus::AwaitingPush,
    }
}

fn current_step(entry: &Entry) -> Option<StepKind> {
    if let Some(run) = &entry.run {
        return Some(run.step);
    }
    match entry.saved.workflow.phase() {
        Phase::AwaitingPush => Some(StepKind::PushSummary),
        _ => entry.saved.workflow.current_step(),
    }
}

fn step_views(entry: &Entry, runs: &[StepRunRow], intake_at: &str) -> Vec<StepView> {
    let wf = &entry.saved.workflow;
    StepKind::ALL
        .iter()
        .map(|&kind| {
            let mut state = wf.state_of(kind);
            if state == StepState::Running && entry.run.as_ref().map(|r| r.step) != Some(kind) {
                state = StepState::Pending;
            }
            let mine: Vec<&StepRunRow> = runs.iter().filter(|r| r.step == step_key(kind)).collect();
            let cost: f64 = mine.iter().filter_map(|r| r.cost_usd).sum();
            let last = mine.last();
            let intake = kind == StepKind::Intake;
            StepView {
                kind,
                index: kind.index(),
                state,
                started_at: last
                    .map(|r| r.started_at.clone())
                    .or_else(|| intake.then(|| intake_at.to_owned())),
                ended_at: last
                    .and_then(|r| r.ended_at.clone())
                    .or_else(|| intake.then(|| intake_at.to_owned())),
                cost_usd: (cost > 0.0).then_some(cost),
                round: (kind == StepKind::CrossReview && wf.round() > 0).then(|| wf.round()),
                model: match &entry.run {
                    Some(r) if r.step == kind => r.model.clone(),
                    _ => last.and_then(|r| r.model.clone()),
                },
                effort: match &entry.run {
                    Some(r) if r.step == kind => r.effort.clone(),
                    _ => last.and_then(|r| r.effort.clone()),
                },
            }
        })
        .collect()
}

fn notes_file(step: StepKind) -> String {
    format!("notes-{}.md", step_key(step))
}

/// Docs/test notes of the latest review of each kind, in workflow order.
fn review_notes(ctx: &TaskCtx) -> Vec<ReviewNote> {
    [
        StepKind::ReviewLocal,
        StepKind::CrossReview,
        StepKind::FinalReview,
    ]
    .into_iter()
    .filter_map(|step| {
        let text = std::fs::read_to_string(ctx.task_dir.join(REVIEW_DIR).join(notes_file(step))).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| ReviewNote {
            step,
            text: text.to_owned(),
        })
    })
    .collect()
}

/// Keep what a review said about docs and tests; a review without it clears the old note.
fn save_review_notes(ctx: &TaskCtx, step: StepKind, answer: &str) {
    let file = ctx.task_dir.join(REVIEW_DIR).join(notes_file(step));
    match text::review_notes(answer) {
        Some(notes) => {
            let _ = std::fs::create_dir_all(ctx.task_dir.join(REVIEW_DIR));
            let _ = std::fs::write(&file, format!("{notes}\n"));
        }
        None => {
            let _ = std::fs::remove_file(&file);
        }
    }
}

/// " Đã review chéo 2 vòng." (nothing before the first round).
fn rounds_note(round: u8) -> String {
    if round == 0 {
        String::new()
    } else {
        format!(" Đã review chéo {round} vòng.")
    }
}

fn gate_message(kind: GateKind, entry: &Entry, findings: &[FindingRow]) -> String {
    let wf = &entry.saved.workflow;
    match kind {
        GateKind::PlanReview => {
            "Plan đã xong: duyệt để AI viết code, góp ý để phân tích lại, hoặc tự code.".into()
        }
        GateKind::Findings => {
            let review = wf.gate_review().unwrap_or(StepKind::FinalReview);
            let open = findings.iter().filter(|f| f.state == "open").count();
            let picked = findings.iter().filter(|f| to_fix(f)).count();
            let next = wf.skip_target().map_or("bước sau", step_label);
            format!(
                "{} báo lỗi: {open} lỗi đang mở, đã chọn {picked} cho AI sửa. Chọn lỗi cần sửa, hoặc sang {next} mà không sửa.{}",
                step_label(review),
                rounds_note(wf.round())
            )
        }
        GateKind::AfterFix => {
            let next = wf.round().saturating_add(1);
            let ask = if wf.round() == 0 {
                "Chạy Review chéo (vòng 1)?".to_owned()
            } else {
                format!("Chạy Review chéo lại (vòng {next})?")
            };
            format!("Sửa lỗi xong. {ask}{}", rounds_note(wf.round()))
        }
        GateKind::SelfCode => {
            "Bạn đang tự code trên nhánh này. Xong thì bấm \"Tôi code xong\" để app review.".into()
        }
        GateKind::AgentQuestion => "AI đang hỏi bạn.".into(),
        GateKind::Error => entry
            .saved
            .error
            .as_ref()
            .map_or_else(|| "Bước vừa chạy bị lỗi.".into(), |e| e.message.clone()),
    }
}

fn gate_view(entry: &Entry, findings: &[FindingRow]) -> Option<GateView> {
    if entry.run.is_some() {
        return None;
    }
    let wf = &entry.saved.workflow;
    match wf.phase() {
        Phase::Gate(kind) => Some(GateView {
            kind,
            since: entry.saved.since.clone().unwrap_or_default(),
            message: gate_message(kind, entry, findings),
            review: wf.gate_review(),
            next: wf.skip_target(),
        }),
        _ => None,
    }
}

fn agent_view(
    wp_id: u64,
    kind: AgentKind,
    status: SessionStatus,
    step: StepKind,
    context: Option<u64>,
    recent_tools: Vec<ToolEvent>,
    claude_handoff: u64,
) -> AgentView {
    let (window, handoff) = if kind == AgentKind::Codex {
        (CODEX_WINDOW, CODEX_HANDOFF)
    } else {
        (CLAUDE_WINDOW, claude_handoff)
    };
    AgentView {
        id: format!("flow-{wp_id}"),
        kind,
        status,
        step,
        context_tokens: context.unwrap_or_default(),
        context_window: window,
        handoff_at: handoff,
        compactions: 0,
        subagents: Vec::new(),
        children: Vec::new(),
        recent_tools,
    }
}

/// Fenced blocks of the push summary become copy buttons; without blocks the whole text is one.
pub fn push_view(text: &str, issue: Option<u64>) -> PushView {
    let mut blocks: Vec<PushBlock> = text::fenced_blocks(text)
        .into_iter()
        .enumerate()
        .filter(|(_, b)| !b.body.trim().is_empty())
        .map(|(i, b)| PushBlock {
            label: if !b.heading.is_empty() {
                b.heading
            } else if !b.lang.is_empty() {
                b.lang
            } else {
                format!("Khối {}", i + 1)
            },
            text: b.body,
        })
        .collect();
    if blocks.is_empty() && !text.trim().is_empty() {
        blocks.push(PushBlock {
            label: "Nội dung".into(),
            text: text.trim().to_owned(),
        });
    }
    PushView {
        issue,
        text: text.to_owned(),
        blocks,
    }
}

fn prune_run_logs(dir: &Path) {
    let max_age = Duration::from_secs(RUN_LOG_DAYS * 24 * 3600);
    let Ok(tasks) = std::fs::read_dir(dir) else { return };
    for task in tasks.flatten() {
        let Ok(files) = std::fs::read_dir(task.path()) else {
            continue;
        };
        for f in files.flatten() {
            let old = f
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > max_age);
            if old {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
}

impl Inner {
    fn lock_tasks(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Entry>> {
        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_store(&self) -> Result<std::sync::MutexGuard<'_, Store>, ApiError> {
        self.store
            .lock()
            .map_err(|_| ApiError::new("storage", "Dữ liệu cục bộ đang bị khoá"))
    }

    fn runs(&self, wp_id: u64) -> Vec<StepRunRow> {
        self.store
            .lock()
            .ok()
            .and_then(|s| s.step_runs(wp_id).ok())
            .unwrap_or_default()
    }

    fn findings(&self, wp_id: u64) -> Vec<FindingRow> {
        self.store
            .lock()
            .ok()
            .and_then(|s| s.findings(wp_id).ok())
            .unwrap_or_default()
    }

    /// Save one task's state. Caller holds the tasks lock (lock order: tasks, then store).
    fn persist(&self, wp_id: u64, entry: &Entry) {
        if let (Ok(json), Ok(store)) = (serde_json::to_string(&entry.saved), self.store.lock()) {
            let _ = store.save_flow(wp_id, &json, &now());
        }
    }

    fn log(&self, wp_id: u64, step: Option<StepKind>, kind: &str, text: &str, keep: bool) {
        let mut tasks = self.lock_tasks();
        if let Some(entry) = tasks.get_mut(&wp_id) {
            push_log(entry, step, kind, text);
        }
        drop(tasks);
        if keep {
            if let Ok(store) = self.store.lock() {
                let _ = store.log_event(&now(), Some(wp_id), &format!("flow.{kind}"), Some(text));
            }
        }
    }

    /// Mark the current step as running. Refuses while another task runs a step in the same repo.
    fn claim(&self, ctx: &TaskCtx) -> Result<Claim, ApiError> {
        let mut tasks = self.lock_tasks();
        let (claim, text) = self.claim_in(&mut tasks, ctx)?;
        drop(tasks);
        if let Ok(store) = self.store.lock() {
            let _ = store.log_event(&now(), Some(ctx.row.wp_id), "flow.info", Some(&text));
        }
        Ok(claim)
    }

    /// [`Inner::claim`] with the tasks lock already held; also returns the log line to keep.
    fn claim_in(&self, tasks: &mut HashMap<u64, Entry>, ctx: &TaskCtx) -> Result<(Claim, String), ApiError> {
        let repo = ctx.row.repo.clone();
        if let Some(repo) = repo.as_deref() {
            let busy = tasks.iter().find_map(|(id, e)| {
                let run = e.run.as_ref()?;
                (*id != ctx.row.wp_id
                    && run
                        .repo
                        .as_deref()
                        .is_some_and(|r| opw_config::same_path(r, repo)))
                .then(|| run.task_id.clone())
            });
            if let Some(other) = busy {
                return Err(ApiError::new(
                    "repo.busy",
                    format!("{other} đang chạy một bước trong repo này. Chờ bước đó xong rồi chạy tiếp."),
                ));
            }
        }
        let entry = tasks.get_mut(&ctx.row.wp_id).ok_or_else(flow_missing)?;
        if entry.run.is_some() {
            return Err(ApiError::new("step.running", "Bước này đang chạy"));
        }
        let Phase::Running(step) = entry.saved.workflow.phase() else {
            return Err(not_now("chạy bước tiếp"));
        };
        let agent = ctx.agent_for(step);
        let choice = model_choice(step, &ctx.config);
        let command = command_label(step, agent, ctx.repo().as_deref());
        let stop = Arc::new(AtomicBool::new(false));
        entry.run = Some(Running {
            step,
            agent,
            task_id: ctx.row.display_id.clone(),
            repo,
            started_at: now(),
            stop: stop.clone(),
            tools: VecDeque::new(),
            context: None,
            model: choice.model.clone(),
            effort: choice.effort.clone(),
            command: command.clone(),
            run_id: None,
        });
        entry.saved.in_flight = true;
        entry.saved.error = None;
        let how = [
            Some(agent_name(agent).to_owned()),
            Some(model_text(choice.model.as_deref(), choice.effort.as_deref())).filter(|m| !m.is_empty()),
            command,
        ];
        let how: Vec<String> = how.into_iter().flatten().collect();
        let text = format!("Bắt đầu {} ({})", step_label(step), how.join(" · "));
        push_log(entry, Some(step), "info", &text);
        self.persist(ctx.row.wp_id, entry);
        Ok((
            Claim {
                step,
                agent,
                stop,
                choice,
            },
            text,
        ))
    }

    /// Record how a step ended and move the state machine. With auto-advance on, the next step
    /// is claimed under the same lock, so the task never shows "ready" between two steps.
    /// `None` = stop here; `Some(Err)` = the next step could not start.
    fn finish(
        &self,
        ctx: &TaskCtx,
        step: StepKind,
        result: Result<Done, FlowError>,
    ) -> Option<Result<Claim, ApiError>> {
        let wp_id = ctx.row.wp_id;
        let mut tasks = self.lock_tasks();
        let entry = tasks.get_mut(&wp_id)?;
        let run_id = entry.run.take().and_then(|r| r.run_id);
        entry.saved.in_flight = false;
        entry.saved.since = Some(now());
        // `note` goes on the run's row (Tiến trình), `text` into the log
        let mut note = String::new();
        let (kind, text) = match result {
            Ok(done) => match entry.saved.workflow.apply(done.event) {
                Ok(()) => {
                    entry.saved.error = None;
                    if step == StepKind::Analyze {
                        entry.saved.feedback = None;
                    }
                    note.clone_from(&done.note);
                    let cost = done.cost.map(|c| format!(" ({})", usd(c))).unwrap_or_default();
                    ("ok", format!("{} xong: {}{cost}", step_label(step), done.note))
                }
                Err(e) => {
                    let err = flow_error("flow.invalid", e.to_string());
                    let _ = entry.saved.workflow.apply(WorkflowEvent::StepFailed(step));
                    let text = err.message.clone();
                    note = short(&err.message);
                    entry.saved.error = Some(err);
                    ("error", text)
                }
            },
            Err(err) => {
                let _ = entry.saved.workflow.apply(WorkflowEvent::StepFailed(step));
                let text = format!("{} lỗi: {}", step_label(step), err.message);
                note = short(&err.message);
                entry.saved.error = Some(err);
                if step.is_fix() {
                    // what was being fixed is open again
                    if let Ok(store) = self.store.lock() {
                        for f in store.findings(wp_id).unwrap_or_default() {
                            if f.state == "fixing" {
                                let _ = store.upsert_finding(&FindingRow {
                                    state: "open".into(),
                                    ..f
                                });
                            }
                        }
                    }
                }
                ("error", text)
            }
        };
        push_log(entry, Some(step), kind, &text);
        self.persist(wp_id, entry);
        let go_on = kind == "ok"
            && ctx.auto(&entry.saved)
            && matches!(entry.saved.workflow.phase(), Phase::Running(_));
        let alert = (!go_on).then(|| stop_alert(ctx, step, entry, &text)).flatten();
        let next = go_on.then(|| self.claim_in(&mut tasks, ctx));
        drop(tasks);
        if let Some(alert) = alert {
            let sink = self.alert_sink.lock().ok().and_then(|s| s.clone());
            if let Some(sink) = sink {
                sink(alert);
            }
        }
        if let Ok(store) = self.store.lock() {
            let _ = store.log_event(&now(), Some(wp_id), &format!("flow.{kind}"), Some(&text));
            if let Some(id) = run_id.filter(|_| !note.is_empty()) {
                let _ = store.set_step_run_note(id, &note);
            }
            if let Some(Ok((_, started))) = &next {
                let _ = store.log_event(&now(), Some(wp_id), "flow.info", Some(started));
            }
        }
        next.map(|n| n.map(|(claim, _)| claim))
    }

    fn on_tool(&self, wp_id: u64, step: StepKind, name: &str, detail: &str) {
        let at = now();
        let text = if detail.is_empty() {
            name.to_owned()
        } else {
            format!("{name} {detail}")
        };
        let mut tasks = self.lock_tasks();
        if let Some(entry) = tasks.get_mut(&wp_id) {
            if let Some(run) = entry.run.as_mut() {
                run.tools.push_back(ToolEvent {
                    at: at.clone(),
                    name: text.clone(),
                });
                while run.tools.len() > TOOLS_SHOWN {
                    run.tools.pop_front();
                }
            }
            push_log(entry, Some(step), "tool", &text);
        }
    }

    /// Claude named its model in the `init` line: show it when the user picked none.
    fn on_model(&self, wp_id: u64, model: &str) {
        if model.is_empty() {
            return;
        }
        if let Some(run) = self.lock_tasks().get_mut(&wp_id).and_then(|e| e.run.as_mut()) {
            run.model.get_or_insert_with(|| model.to_owned());
        }
    }

    fn on_context(&self, wp_id: u64, tokens: u64) {
        if let Some(run) = self.lock_tasks().get_mut(&wp_id).and_then(|e| e.run.as_mut()) {
            run.context = Some(tokens);
        }
    }

    fn on_rate_limit(&self, event: &serde_json::Value) {
        let sink = self.quota_sink.lock().ok().and_then(|s| s.clone());
        if let (Some(sink), Some(snap)) = (sink, opw_quota::from_claude_rate_limit_event(event, &now())) {
            sink(snap);
        }
    }

    /// Create the run's raw log, starting with the prompt the app sends (the Chat tab shows it);
    /// returns the file and its name relative to the runs folder.
    fn open_log(&self, ctx: &TaskCtx, step: StepKind, job: &Job) -> (Option<std::fs::File>, Option<String>) {
        use std::io::Write;
        let Some(runs) = self.runs_dir.as_ref() else {
            return (None, None);
        };
        let stamp = now().replace([':', '.'], "-");
        let name = format!("{}/{stamp}-{}.jsonl", ctx.row.display_id, step_key(step));
        let path = runs.join(&name);
        let file = path
            .parent()
            .and_then(|d| std::fs::create_dir_all(d).ok())
            .and_then(|()| std::fs::File::create(&path).ok());
        match file {
            Some(mut f) => {
                let _ = writeln!(f, "{}", transcript::prompt_line(&prompt_of(job)));
                (Some(f), Some(name))
            }
            None => (None, None),
        }
    }

    /// Start the run's `step_run` row and remember its id on the running step.
    fn begin_run(&self, ctx: &TaskCtx, step: StepKind, agent: &str, log: Option<&str>) -> Option<i64> {
        let wp_id = ctx.row.wp_id;
        let round = (step == StepKind::CrossReview).then(|| self.round_of(wp_id));
        let id = self.store.lock().ok().and_then(|s| {
            s.begin_step_run(wp_id, step_key(step), agent, round, &now(), log)
                .ok()
        });
        if let Some(run) = self.lock_tasks().get_mut(&wp_id).and_then(|e| e.run.as_mut()) {
            run.run_id = id;
        }
        id
    }

    /// Run Claude for one step; the final `result` line comes back.
    fn run_claude(
        &self,
        ctx: &TaskCtx,
        step: StepKind,
        job: &Job,
        claim: &Claim,
    ) -> Result<RunResult, FlowError> {
        let stop: &AtomicBool = &claim.stop;
        use std::io::Write;
        let wp_id = ctx.row.wp_id;
        let (budget, timeout, key) = limits(step, &ctx.config);
        let (mut raw, log) = self.open_log(ctx, step, job);
        let run_id = self.begin_run(ctx, step, "claude", log.as_deref());
        let mut result: Option<RunResult> = None;
        let mut session = String::new();
        let mut used_model = String::new();
        let mut on_line = |line: &str| {
            if let Some(f) = raw.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            for event in claude::parse_line(line).unwrap_or_default() {
                match event {
                    ClaudeEvent::Init {
                        session_id, model, ..
                    } => {
                        session = session_id;
                        self.on_model(wp_id, &model);
                        used_model = model;
                    }
                    ClaudeEvent::ToolUse { name, detail } => self.on_tool(wp_id, step, &name, &detail),
                    ClaudeEvent::Context(tokens) => self.on_context(wp_id, tokens),
                    ClaudeEvent::RateLimit(v) => self.on_rate_limit(&v),
                    ClaudeEvent::Result(r) => result = Some(r),
                    ClaudeEvent::AssistantText | ClaudeEvent::Other(_) => {}
                }
            }
        };
        let launched = self.launcher.launch(job, stop, &mut on_line);
        let ok = matches!(&launched, Ok(o) if o.success()) && result.as_ref().is_some_and(|r| !r.is_error);
        let state = if ok { "done" } else { "failed" };
        if let (Some(id), Ok(store)) = (run_id, self.store.lock()) {
            let r = result.as_ref();
            let sid = r
                .map(|r| r.session_id.as_str())
                .filter(|s| !s.is_empty())
                .or(Some(session.as_str()).filter(|s| !s.is_empty()));
            let _ = store.end_step_run(
                id,
                &StepRunEnd {
                    state,
                    session_id: sid,
                    cost_usd: r.map(|r| r.cost_usd),
                    tokens_in: r.map(|r| r.tokens_in),
                    tokens_out: r.map(|r| r.tokens_out),
                    ended_at: &now(),
                    model: Some(used_model.as_str())
                        .filter(|m| !m.is_empty())
                        .or(claim.choice.model.as_deref()),
                    effort: claim.choice.effort.as_deref(),
                    denials: r.and_then(|r| denials_json(&r.denials)).as_deref(),
                },
            );
        }
        if let Some(r) = &result {
            for d in &r.denials {
                self.log(
                    wp_id,
                    Some(step),
                    "info",
                    &format!("Bị từ chối (ngoài allowlist): {}", d.line()),
                    false,
                );
            }
        }
        let outcome = match launched {
            Ok(o) => o,
            Err(e) => return Err(launch_failed(AgentKind::Claude, &e)),
        };
        if let Some(err) = stop_error(&outcome, timeout, key) {
            return Err(err);
        }
        match result {
            Some(r) if r.over_budget() => Err(flow_error(
                "step.budget",
                format!(
                    "Bước này chạm giới hạn ${budget}. Tăng `steps.{key}.maxBudgetUsd` trong config.json nếu cần, rồi bấm Thử lại."
                ),
            )),
            Some(r) if r.is_error || !outcome.success() => {
                let why = if r.text.trim().is_empty() {
                    outcome.stderr_tail.clone()
                } else {
                    r.text.clone()
                };
                if looks_signed_out(&why) || looks_signed_out(&outcome.stderr_tail) {
                    Err(signed_out(AgentKind::Claude))
                } else {
                    Err(flow_error(
                        "step.failed",
                        format!("Claude báo lỗi ({}): {}", r.subtype, short(&why)),
                    ))
                }
            }
            Some(r) => Ok(r),
            None if looks_signed_out(&outcome.stderr_tail) => Err(signed_out(AgentKind::Claude)),
            None => Err(flow_error(
                "step.failed",
                format!("Claude dừng mà không trả kết quả: {}", exit_why(&outcome)),
            )),
        }
    }

    /// Run Codex for one step; its last message comes back.
    fn run_codex(
        &self,
        ctx: &TaskCtx,
        step: StepKind,
        job: &Job,
        claim: &Claim,
    ) -> Result<String, FlowError> {
        let stop: &AtomicBool = &claim.stop;
        use std::io::Write;
        let wp_id = ctx.row.wp_id;
        let (_, timeout, key) = limits(step, &ctx.config);
        let (mut raw, log) = self.open_log(ctx, step, job);
        let run_id = self.begin_run(ctx, step, "codex", log.as_deref());
        let mut thread = String::new();
        let mut last = String::new();
        let mut failed: Option<String> = None;
        let (mut tokens_in, mut tokens_out) = (0_u64, 0_u64);
        let mut on_line = |line: &str| {
            if let Some(f) = raw.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            match codex::parse_line(line) {
                Ok(CodexEvent::Started { thread_id }) => thread = thread_id,
                Ok(CodexEvent::Command(cmd)) => self.on_tool(wp_id, step, "Shell", &cmd),
                Ok(CodexEvent::Message(m)) => last = m,
                Ok(CodexEvent::Usage { input, output, .. }) => {
                    tokens_in += input;
                    tokens_out += output;
                    self.on_context(wp_id, input);
                }
                Ok(CodexEvent::Failed(m)) => failed = Some(m),
                _ => {}
            }
        };
        let launched = self.launcher.launch(job, stop, &mut on_line);
        let ok = matches!(&launched, Ok(o) if o.success()) && failed.is_none();
        if let (Some(id), Ok(store)) = (run_id, self.store.lock()) {
            let _ = store.end_step_run(
                id,
                &StepRunEnd {
                    state: if ok { "done" } else { "failed" },
                    session_id: Some(thread.as_str()).filter(|t| !t.is_empty()),
                    cost_usd: None,
                    tokens_in: Some(tokens_in),
                    tokens_out: Some(tokens_out),
                    ended_at: &now(),
                    model: claim.choice.model.as_deref(),
                    effort: claim.choice.effort.as_deref(),
                    denials: None,
                },
            );
        }
        let outcome = launched.map_err(|e| launch_failed(AgentKind::Codex, &e))?;
        if let Some(err) = stop_error(&outcome, timeout, key) {
            return Err(err);
        }
        if let Some(m) = failed {
            if looks_signed_out(&m) || looks_signed_out(&outcome.stderr_tail) {
                return Err(signed_out(AgentKind::Codex));
            }
            return Err(flow_error("step.failed", format!("Codex báo lỗi: {}", short(&m))));
        }
        if !outcome.success() {
            if looks_signed_out(&outcome.stderr_tail) {
                return Err(signed_out(AgentKind::Codex));
            }
            return Err(flow_error(
                "step.failed",
                format!("Codex dừng lỗi: {}", exit_why(&outcome)),
            ));
        }
        Ok(last)
    }

    fn round_of(&self, wp_id: u64) -> u8 {
        self.lock_tasks()
            .get(&wp_id)
            .map(|e| e.saved.workflow.round())
            .unwrap_or_default()
    }

    /// Read some saved fields and maybe change them, then save.
    fn with_saved<T>(&self, wp_id: u64, f: impl FnOnce(&mut Saved) -> T) -> Option<T> {
        let mut tasks = self.lock_tasks();
        let entry = tasks.get_mut(&wp_id)?;
        let out = f(&mut entry.saved);
        self.persist(wp_id, entry);
        Some(out)
    }

    fn merge(
        &self,
        ctx: &TaskCtx,
        step: StepKind,
        round: u8,
        source: &str,
        raw: &[RawFinding],
    ) -> Result<Vec<FindingRow>, FlowError> {
        let store = self
            .store
            .lock()
            .map_err(|_| flow_error("storage", "Dữ liệu cục bộ đang bị khoá"))?;
        merge_findings(&store, ctx.row.wp_id, step, round, source, raw, &now())
            .and_then(|()| store.findings(ctx.row.wp_id))
            .map_err(|e| flow_error("storage", e.to_string()))
    }
}

fn launch_failed(agent: AgentKind, e: &LaunchError) -> FlowError {
    let (cli, install) = match agent {
        AgentKind::Codex => ("codex", "Codex CLI (npm i -g @openai/codex)"),
        _ => ("claude", "Claude Code"),
    };
    match e {
        LaunchError::Missing => FlowError {
            code: "agent.missing".into(),
            message: format!("Không thấy lệnh `{cli}` trên PATH. Cài {install} rồi bấm Thử lại."),
            provider: Some(provider_of(agent)),
        },
        LaunchError::Spawn(e) => flow_error("step.failed", format!("Không chạy được {cli}: {e}")),
    }
}

fn stop_error(outcome: &RunOutcome, timeout: Duration, key: &str) -> Option<FlowError> {
    if outcome.stopped {
        return Some(flow_error(
            "step.stopped",
            "Bạn đã dừng bước này. Bấm Thử lại để chạy lại.",
        ));
    }
    if outcome.timed_out {
        return Some(flow_error(
            "step.timeout",
            format!(
                "Bước chạy quá {} phút nên app đã dừng nó. Tăng `steps.{key}.timeoutMin` nếu cần, rồi bấm Thử lại.",
                timeout.as_secs() / 60
            ),
        ));
    }
    None
}

fn exit_why(outcome: &RunOutcome) -> String {
    if outcome.stderr_tail.trim().is_empty() {
        format!(
            "mã thoát {}",
            outcome.code.map_or_else(|| "?".into(), |c| c.to_string())
        )
    } else {
        short(&outcome.stderr_tail)
    }
}

fn unreadable_review() -> FlowError {
    flow_error(
        "findings.parse",
        "Không đọc được kết quả review (không có khối JSON hay mốc 🔴/🟠/🟡/🔵). Xem Log rồi bấm Thử lại.",
    )
}

fn write_task_file(ctx: &TaskCtx, name: &str, text: &str) -> Result<(), FlowError> {
    std::fs::create_dir_all(&ctx.task_dir)
        .and_then(|()| std::fs::write(ctx.file(name), format!("{}\n", text.trim_end())))
        .map_err(|e| flow_error("task.write", format!("Không ghi được {name}: {e}")))
}

fn usd(cost: f64) -> String {
    format!("${cost:.2}")
}

/// The state-machine event of a finished review, and a line for the log. What it reported and
/// the user has not accepted or deferred before stops the flow at the findings gate.
fn review_event(findings: &[FindingRow], raw: &[RawFinding]) -> (WorkflowEvent, String) {
    let reported: HashSet<String> = raw.iter().map(|r| fingerprint(&r.file, &r.title)).collect();
    let open = findings
        .iter()
        .filter(|f| reported.contains(&f.fingerprint) && !settled(&f.state))
        .count();
    let decided = reported.len().saturating_sub(open);
    let mut by_severity = Vec::new();
    for sev in ["critical", "high", "medium", "low"] {
        let n = raw
            .iter()
            .filter(|r| severity_of(&r.severity) == severity_of(sev))
            .count();
        if n > 0 {
            by_severity.push(format!("{n} {sev}"));
        }
    }
    let found = if raw.is_empty() {
        "không thấy lỗi".to_owned()
    } else {
        format!("báo {} lỗi ({})", raw.len(), by_severity.join(", "))
    };
    let note = if decided > 0 {
        format!("{found}, {decided} lỗi bạn đã chấp nhận / để sau từ trước")
    } else {
        found
    };
    (WorkflowEvent::ReviewFinished { open }, note)
}

/// Check the repo, run the step, check the repo again.
fn execute(inner: &Inner, ctx: &TaskCtx, claim: &Claim) -> Result<Done, FlowError> {
    let step = claim.step;
    let repo = ctx.repo().filter(|p| p.is_dir()).ok_or_else(|| {
        flow_error(
            "repo.missing",
            "Không thấy thư mục repo của task (đã xoá hoặc đổi chỗ?)",
        )
    })?;
    let git = Git::new(&repo);
    if let Some(branch) = ctx.row.branch.as_deref() {
        let current = git
            .current_branch()
            .map_err(|e| flow_error("repo.git", e.to_string()))?;
        if current != branch {
            return Err(flow_error(
                "repo.branch",
                format!(
                    "Repo đang ở nhánh {current}, task này làm trên {branch}. Switch về {branch} (commit hoặc stash trước nếu cần) rồi bấm Thử lại."
                ),
            ));
        }
    }
    let before = git_snap(&repo).map_err(|e| flow_error("repo.git", e))?;
    let result = run_step(inner, ctx, claim, &repo);
    // a surprise in git matters more than how the step itself ended
    if let Ok(after) = git_snap(&repo) {
        if let Some(what) = git_surprise(step, &before, &after) {
            return Err(flow_error(
                "git.unexpected",
                format!(
                    "App dừng vì {what} trong lúc {} chạy. Kiểm tra repo (git status, git log) trước khi chạy tiếp; app không tự sửa gì.",
                    step_label(step)
                ),
            ));
        }
    }
    result
}

fn run_step(inner: &Inner, ctx: &TaskCtx, claim: &Claim, repo: &Path) -> Result<Done, FlowError> {
    use WorkflowEvent as E;
    let step = claim.step;
    let wp_id = ctx.row.wp_id;
    let (budget, timeout, _) = limits(step, &ctx.config);
    let base = ctx
        .row
        .base
        .clone()
        .ok_or_else(|| flow_error("task.base", "Task chưa có nhánh gốc (base)"))?;
    let brief = ctx
        .read(BRIEF_FILE)
        .ok_or_else(|| flow_error("brief.missing", "Không đọc được brief.md của task"))?;
    let job = |agent: Provider, args: Vec<String>, stdin: Option<String>| Job {
        agent,
        args,
        cwd: repo.to_path_buf(),
        stdin,
        timeout,
    };
    let (session, feedback, fixes, issue, note) = inner
        .with_saved(wp_id, |s| {
            (
                s.session.clone(),
                s.feedback.clone(),
                s.fixes,
                s.issue,
                s.note.clone(),
            )
        })
        .unwrap_or_default();
    let set_session = |id: &str| {
        if !id.is_empty() {
            inner.with_saved(wp_id, |s| s.session = Some(id.to_owned()));
        }
    };
    // the user's model / effort for this step (nothing when they left it to the machine)
    let cl = |args: Vec<String>| claude::with_model(args, &claim.choice);
    match step {
        StepKind::Intake => Ok(Done {
            event: E::StepSucceeded(StepKind::Intake),
            note: String::new(),
            cost: None,
        }),
        StepKind::Analyze => {
            let previous_plan = ctx.read(PLAN_FILE);
            let previous = feedback.as_deref().zip(previous_plan.as_deref());
            let prompt = prompts::analyze(&brief, previous);
            let r = inner.run_claude(
                ctx,
                step,
                &job(Provider::Claude, cl(claude::analyze_args(budget)), Some(prompt)),
                claim,
            )?;
            let plan = r.text.trim();
            if plan.is_empty() {
                return Err(flow_error(
                    "plan.empty",
                    "Claude không trả về plan. Xem Log rồi bấm Thử lại.",
                ));
            }
            write_task_file(ctx, PLAN_FILE, plan)?;
            set_session(&r.session_id);
            Ok(Done {
                event: E::StepSucceeded(StepKind::Analyze),
                note: plan_note(plan),
                cost: Some(r.cost_usd),
            })
        }
        StepKind::Implement => {
            let plan = ctx
                .read(PLAN_FILE)
                .ok_or_else(|| flow_error("plan.missing", "Không đọc được plan.md"))?;
            let allowed = ctx.repo_config().allow_bash;
            let prompt = prompts::implement(
                &plan,
                session.is_none().then_some(brief.as_str()),
                &allowed,
                note.as_deref(),
            );
            let args = cl(claude::code_args(session.as_deref(), budget, &allowed));
            let r = inner.run_claude(ctx, step, &job(Provider::Claude, args, Some(prompt)), claim)?;
            set_session(&r.session_id);
            if note.is_some() {
                inner.with_saved(wp_id, |s| s.note = None);
            }
            Ok(Done {
                event: E::StepSucceeded(StepKind::Implement),
                note: format!("{}{}", change_note(repo), denied_note(r.denials.len())),
                cost: Some(r.cost_usd),
            })
        }
        StepKind::ReviewLocal | StepKind::FinalReview => {
            let has_command = repo
                .join(".claude")
                .join("commands")
                .join("review-local.md")
                .is_file();
            let (args, stdin, source) = if has_command {
                (claude::review_local_args(&base, budget), None, "review-local")
            } else {
                (
                    claude::review_args(budget),
                    Some(prompts::review(&base, &brief)),
                    "claude",
                )
            };
            let r = inner.run_claude(ctx, step, &job(Provider::Claude, cl(args), stdin), claim)?;
            let raw = text::findings_from_text(&r.text).ok_or_else(unreadable_review)?;
            save_review_notes(ctx, step, &r.text);
            let findings = inner.merge(ctx, step, 0, source, &raw)?;
            let (event, note) = review_event(&findings, &raw);
            Ok(Done {
                event,
                note,
                cost: Some(r.cost_usd),
            })
        }
        StepKind::Fix | StepKind::FixCross => {
            let findings = inner.findings(wp_id);
            let items: Vec<&FindingRow> = findings.iter().filter(|f| to_fix(f)).collect();
            if items.is_empty() {
                return Ok(Done {
                    event: E::StepSucceeded(step),
                    note: "không có lỗi nào được chọn để sửa".into(),
                    cost: None,
                });
            }
            if let Ok(store) = inner.store.lock() {
                for f in &items {
                    let _ = store.upsert_finding(&FindingRow {
                        state: "fixing".into(),
                        ..(*f).clone()
                    });
                }
            }
            let list: Vec<FixItem> = items
                .iter()
                .map(|f| FixItem {
                    severity: f.severity.clone(),
                    file: f.file.clone(),
                    line: f.line,
                    title: f.title.clone(),
                    detail: f.detail.clone().unwrap_or_default(),
                    suggestion: f.suggestion.clone().unwrap_or_default(),
                })
                .collect();
            let allowed = ctx.repo_config().allow_bash;
            let args = cl(claude::code_args(session.as_deref(), budget, &allowed));
            let r = inner.run_claude(
                ctx,
                step,
                &job(Provider::Claude, args, Some(prompts::fix(&list, &allowed))),
                claim,
            )?;
            set_session(&r.session_id);
            let fixes = inner
                .with_saved(wp_id, |s| {
                    s.fixes += 1;
                    s.fixes
                })
                .unwrap_or(fixes + 1);
            Ok(Done {
                event: E::StepSucceeded(step),
                note: format!(
                    "lần sửa thứ {fixes}: xử lý {} lỗi; {}{}",
                    list.len(),
                    change_note(repo),
                    denied_note(r.denials.len())
                ),
                cost: Some(r.cost_usd),
            })
        }
        // turned on by later parts; until then the workflow skips them
        StepKind::Checks | StepKind::Harness | StepKind::BrowserTest => Err(flow_error(
            "step.notYet",
            format!("{} chưa chạy được ở bản này", step_label(step)),
        )),
        StepKind::CrossReview => {
            let round = inner.round_of(wp_id);
            let (raw, source, cost) = if claim.agent == AgentKind::Codex {
                let dir = ctx.task_dir.join(REVIEW_DIR);
                std::fs::create_dir_all(&dir).map_err(|e| flow_error("task.write", e.to_string()))?;
                let schema = dir.join("findings.schema.json");
                std::fs::write(&schema, codex::FINDINGS_SCHEMA)
                    .map_err(|e| flow_error("task.write", e.to_string()))?;
                let out = dir.join(format!("codex-r{round}.json"));
                let _ = std::fs::remove_file(&out);
                let args = codex::with_model(
                    codex::cross_review_args(&schema.to_string_lossy(), &out.to_string_lossy()),
                    &claim.choice,
                );
                let answer = inner.run_codex(
                    ctx,
                    step,
                    &job(Provider::Codex, args, Some(prompts::cross_review(&base, &brief))),
                    claim,
                )?;
                let text = std::fs::read_to_string(&out).unwrap_or(answer);
                let raw = serde_json::from_str::<serde_json::Value>(text.trim())
                    .ok()
                    .and_then(|v| text::findings_from_json(&v))
                    .or_else(|| text::findings_from_text(&text))
                    .ok_or_else(unreadable_review)?;
                save_review_notes(ctx, step, &text);
                (raw, "codex", None)
            } else {
                let stdin = Some(prompts::review(&base, &brief));
                let r = inner.run_claude(
                    ctx,
                    step,
                    &job(Provider::Claude, cl(claude::review_args(budget)), stdin),
                    claim,
                )?;
                let raw = text::findings_from_text(&r.text).ok_or_else(unreadable_review)?;
                save_review_notes(ctx, step, &r.text);
                (raw, "claude", Some(r.cost_usd))
            };
            let findings = inner.merge(ctx, step, round, source, &raw)?;
            let (event, note) = review_event(&findings, &raw);
            Ok(Done {
                event,
                note: format!("vòng {round}: {note}"),
                cost,
            })
        }
        StepKind::PushSummary => {
            let issue = issue.or_else(|| {
                crate::issues::github_links(&brief)
                    .iter()
                    .find_map(|u| text::issue_number(u))
            });
            inner.with_saved(wp_id, |s| s.issue = issue);
            let has_command = repo
                .join(".claude")
                .join("commands")
                .join("push-summary.md")
                .is_file();
            let (args, stdin) = if has_command {
                (claude::push_summary_args(issue, budget), None)
            } else {
                (
                    claude::push_summary_default_args(budget),
                    Some(prompts::push_summary(&base, issue)),
                )
            };
            let r = inner.run_claude(ctx, step, &job(Provider::Claude, cl(args), stdin), claim)?;
            if r.text.trim().is_empty() {
                return Err(flow_error(
                    "push.empty",
                    "Claude không trả về commit message. Xem Log rồi bấm Thử lại.",
                ));
            }
            write_task_file(ctx, PUSH_FILE, &r.text)?;
            Ok(Done {
                event: E::StepSucceeded(StepKind::PushSummary),
                note: "đã viết commit message + PR description".into(),
                cost: Some(r.cost_usd),
            })
        }
    }
}

/// Runs the claimed step, then the next ones while auto-advance is on.
fn worker(inner: &Arc<Inner>, ctx: &TaskCtx, first: Claim) {
    let mut claim = first;
    loop {
        let step = claim.step;
        let result = execute(inner, ctx, &claim);
        match inner.finish(ctx, step, result) {
            None => return,
            Some(Ok(next)) => claim = next,
            Some(Err(e)) => {
                inner.log(
                    ctx.row.wp_id,
                    None,
                    "error",
                    &format!("Không chạy tiếp được: {}", e.message),
                    true,
                );
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// How the fake AI answers one job.
    trait ScriptFn: Fn(&Job, &AtomicBool, &mut dyn FnMut(&str)) -> RunOutcome + Send + Sync {}
    impl<F: Fn(&Job, &AtomicBool, &mut dyn FnMut(&str)) -> RunOutcome + Send + Sync> ScriptFn for F {}
    type Script = dyn ScriptFn;

    /// Plays the AI: records every job and answers through `script`.
    struct Fake {
        script: Box<Script>,
        jobs: Mutex<Vec<Job>>,
        missing: bool,
    }

    impl Launcher for Fake {
        fn launch(
            &self,
            job: &Job,
            stop: &AtomicBool,
            on_line: &mut dyn FnMut(&str),
        ) -> Result<RunOutcome, LaunchError> {
            self.jobs.lock().unwrap().push(job.clone());
            if self.missing {
                return Err(LaunchError::Missing);
            }
            Ok((self.script)(job, stop, on_line))
        }
    }

    fn ok() -> RunOutcome {
        RunOutcome {
            code: Some(0),
            ..RunOutcome::default()
        }
    }

    fn result_line(text: &str) -> String {
        serde_json::json!({
            "type": "result", "subtype": "success", "is_error": false, "result": text,
            "session_id": "s-1", "total_cost_usd": 0.25,
            "usage": {"input_tokens": 100, "output_tokens": 20}
        })
        .to_string()
    }

    fn init_line() -> String {
        r#"{"type":"system","subtype":"init","session_id":"s-1","model":"m","tools":["Read"]}"#.to_owned()
    }

    fn tool_line() -> String {
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"a.ts"}}],"usage":{"input_tokens":10,"cache_read_input_tokens":5000}}}"#.to_owned()
    }

    /// What kind of run a job is, from its arguments.
    fn kind(job: &Job) -> &'static str {
        let has = |flag: &str| job.args.iter().any(|a| a == flag);
        if job.agent == Provider::Codex {
            "codex"
        } else if has("--restricted") {
            "analyze"
        } else if has("acceptEdits") {
            "code"
        } else if has("--no-session-persistence") {
            "push"
        } else {
            "review"
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    struct Setup {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        store: Arc<Mutex<Store>>,
        tasks: PathBuf,
    }

    impl Setup {
        fn ctx(&self, wp_id: u64, config: &Config) -> TaskCtx {
            let id = format!("WP-{wp_id}");
            let task_dir = self.tasks.join(&id);
            std::fs::create_dir_all(&task_dir).unwrap();
            std::fs::write(
                task_dir.join(BRIEF_FILE),
                format!("# [{id}] Fix org\nIssue: https://github.com/o/r/issues/279\n"),
            )
            .unwrap();
            let row = TaskRow {
                wp_id,
                display_id: id,
                title: "Fix org".into(),
                project: "Web".into(),
                repo: Some(self.repo.to_string_lossy().into_owned()),
                branch: Some("f/WP-1".into()),
                base: Some("main".into()),
                column: "active".into(),
                updated_at: "2026-11-14T08:00:00Z".into(),
            };
            self.store.lock().unwrap().upsert_task(&row).unwrap();
            TaskCtx {
                row,
                task_dir,
                config: config.clone(),
            }
        }
    }

    fn setup() -> Option<Setup> {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git not found; skipping");
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("web");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.ts"), "export const a = 1;\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        git(&repo, &["switch", "-q", "-c", "f/WP-1"]);
        Some(Setup {
            repo,
            tasks: tmp.path().join("tasks"),
            store: Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
            _tmp: tmp,
        })
    }

    fn flow_with(s: &Setup, missing: bool, script: impl ScriptFn + 'static) -> (Flow, Arc<Fake>) {
        let fake = Arc::new(Fake {
            script: Box::new(script),
            jobs: Mutex::new(Vec::new()),
            missing,
        });
        (Flow::new(s.store.clone(), fake.clone(), None), fake)
    }

    /// Wait until no step runs for this task.
    fn settle(flow: &Flow, ctx: &TaskCtx) -> TaskDetail {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let d = flow.detail(ctx);
            if d.status != FlowStatus::Running {
                return d;
            }
            assert!(Instant::now() < deadline, "the step did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn run(flow: &Flow, ctx: &TaskCtx, action: FlowAction) -> TaskDetail {
        flow.act(ctx.clone(), action).unwrap();
        settle(flow, ctx)
    }

    const HIGH: &str = "Review\n```json\n{\"findings\":[{\"severity\":\"high\",\"file\":\"a.ts\",\"line\":1,\"title\":\"Null org\",\"detail\":\"crash\",\"suggestion\":\"guard\"}]}\n```";
    const CLEAN: &str = "```json\n{\"findings\":[]}\n```";
    const MEDIUM: &str = "```json\n{\"findings\":[{\"severity\":\"medium\",\"file\":\"a.ts\",\"line\":1,\"title\":\"Missing test\",\"detail\":\"\",\"suggestion\":\"\"}]}\n```";
    const PUSH: &str = "### Commit message\n```text\nfix: org (#279)\n```\n### PR description\n```markdown\n## Summary\n- guard\n```";

    /// Answers like a real run: analyze writes a plan, code steps edit a.ts, the first Claude
    /// review finds one high, Codex and the final review are clean.
    fn happy(final_review: &'static str) -> impl ScriptFn {
        let reviews = AtomicUsize::new(0);
        move |job, _stop, out| {
            match kind(job) {
                "analyze" => {
                    out(&init_line());
                    out(&tool_line());
                    out(&result_line("## Nguyên nhân\n`a.ts:1`\n## Đề xuất\nguard"));
                }
                "code" => {
                    let file = job.cwd.join("a.ts");
                    let old = std::fs::read_to_string(&file).unwrap();
                    std::fs::write(&file, format!("{old}// edit\n")).unwrap();
                    out(&result_line("đã sửa a.ts"));
                }
                "review" => {
                    let n = reviews.fetch_add(1, Ordering::SeqCst);
                    out(&result_line(if n == 0 { HIGH } else { final_review }));
                }
                "codex" => {
                    out(r#"{"type":"thread.started","thread_id":"t"}"#);
                    out(
                        r#"{"type":"item.started","item":{"type":"command_execution","command":"git diff main"}}"#,
                    );
                    out(
                        r#"{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":50}}"#,
                    );
                    let i = job.args.iter().position(|a| a == "-o").unwrap();
                    std::fs::write(&job.args[i + 1], "{\"findings\":[]}").unwrap();
                }
                "push" => out(&result_line(PUSH)),
                _ => unreachable!(),
            }
            ok()
        }
    }

    /// The user picks every open finding for the AI to fix.
    fn pick_all(flow: &Flow, ctx: &TaskCtx) {
        for f in flow.detail(ctx).findings {
            if f.state == opw_domain::finding::FindingState::Open {
                flow.decide(&FindingDecision {
                    wp_id: ctx.row.wp_id,
                    fingerprint: f.fingerprint,
                    decision: Decision::Fix,
                    reason: None,
                })
                .unwrap();
            }
        }
    }

    /// Answer every stop the plain way (approve, no fixes, no new cross review) until the push
    /// summary is ready.
    fn to_push(flow: &Flow, ctx: &TaskCtx, mut d: TaskDetail) -> TaskDetail {
        for _ in 0..12 {
            let action = match (d.status, d.gate.as_ref().map(|g| g.kind)) {
                (FlowStatus::AwaitingPush, _) => return d,
                (FlowStatus::Gate, Some(GateKind::PlanReview)) => FlowAction::ApprovePlan,
                (FlowStatus::Gate, Some(GateKind::Findings)) => FlowAction::PickFindings { fix: false },
                (FlowStatus::Gate, Some(GateKind::AfterFix)) => FlowAction::AfterFix {
                    choice: AfterFix::Next,
                },
                (FlowStatus::Ready, _) => FlowAction::Run,
                other => panic!("{other:?}: {:?}", d.error),
            };
            d = run(flow, ctx, action);
        }
        d
    }

    #[test]
    fn runs_all_steps_and_stops_at_each_one() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, happy(CLEAN));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let d = flow.detail(&ctx);
        assert_eq!(
            (d.status, d.current),
            (FlowStatus::Ready, Some(StepKind::Analyze))
        );
        assert!(!d.auto_advance);

        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.status, FlowStatus::Gate, "{:?}", d.error);
        assert_eq!(d.gate.as_ref().map(|g| g.kind), Some(GateKind::PlanReview));
        assert!(d.plan.as_deref().unwrap().starts_with("## Nguyên nhân"));
        assert!(d.log.iter().any(|l| l.kind == "tool" && l.text == "Read a.ts"));

        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(
            (d.status, d.current),
            (FlowStatus::Ready, Some(StepKind::ReviewLocal)),
            "{:?}",
            d.error
        );
        assert!(
            std::fs::read_to_string(s.repo.join("a.ts"))
                .unwrap()
                .contains("// edit")
        );

        // the review found a high: the flow waits for the user to pick what gets fixed
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.current, Some(StepKind::ReviewLocal), "{:?}", d.error);
        let gate = d.gate.clone().unwrap();
        assert_eq!(
            (gate.kind, gate.review, gate.next),
            (
                GateKind::Findings,
                Some(StepKind::ReviewLocal),
                Some(StepKind::CrossReview)
            )
        );
        assert!(
            gate.message
                .starts_with("Tự review báo lỗi: 1 lỗi đang mở, đã chọn 0"),
            "{}",
            gate.message
        );
        assert_eq!(d.findings.len(), 1);
        assert_eq!(d.findings[0].severity, Severity::High);
        assert_eq!(d.findings[0].detail.as_deref(), Some("crash"));
        assert_eq!(d.findings[0].source, "claude");
        assert_eq!(d.findings[0].step, Some(StepKind::ReviewLocal));
        let card = flow.card(&ctx).unwrap();
        assert_eq!(
            (card.step.kind, card.step.state, card.gate),
            (
                StepKind::ReviewLocal,
                StepState::WaitingUser,
                Some(GateKind::Findings)
            )
        );
        // a high is not fixed unless the user picks it
        assert_eq!(
            flow.act(ctx.clone(), FlowAction::PickFindings { fix: true })
                .unwrap_err()
                .code,
            "findings.none"
        );
        pick_all(&flow, &ctx);
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        // the fix ran; now: review again?
        let gate = d.gate.clone().unwrap();
        assert_eq!(
            (gate.kind, d.current, gate.next),
            (
                GateKind::AfterFix,
                Some(StepKind::Fix),
                Some(StepKind::FinalReview)
            ),
            "{:?}",
            d.error
        );
        assert_eq!(gate.message, "Sửa lỗi xong. Chạy Review chéo (vòng 1)?");
        assert_eq!(d.findings[0].state, opw_domain::finding::FindingState::Fixing);
        assert!(
            d.runs
                .last()
                .unwrap()
                .note
                .as_deref()
                .unwrap()
                .starts_with("lần sửa thứ 1: xử lý 1 lỗi"),
            "{:?}",
            d.runs.last()
        );

        let d = run(
            &flow,
            &ctx,
            FlowAction::AfterFix {
                choice: AfterFix::CrossReview,
            },
        );
        assert_eq!(
            (d.status, d.current),
            (FlowStatus::Ready, Some(StepKind::FinalReview)),
            "{:?}",
            d.error
        );
        assert_eq!(d.round, 1);
        assert_eq!(d.findings[0].state, opw_domain::finding::FindingState::Fixed);
        let system = flow.system(&ctx).unwrap();
        assert_eq!(system.agents[0].status, SessionStatus::Idle);
        let cross = system
            .steps
            .iter()
            .find(|s| s.kind == StepKind::CrossReview)
            .unwrap();
        assert_eq!(cross.round, Some(1));

        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.current, Some(StepKind::PushSummary), "{:?}", d.error);
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);
        let push = d.push.unwrap();
        assert_eq!(push.issue, Some(279));
        assert_eq!(
            push.blocks.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(),
            ["Commit message", "PR description"]
        );
        assert_eq!(flow.card(&ctx).unwrap().column, TaskColumn::AwaitingPush);
        assert!(d.cost_usd > 1.0, "{}", d.cost_usd);

        let jobs = fake.jobs.lock().unwrap().clone();
        assert_eq!(
            jobs.iter().map(kind).collect::<Vec<_>>(),
            ["analyze", "code", "review", "code", "codex", "review", "push"]
        );
        // implement and fix continue the analyze session; the codex review is read-only
        assert!(
            jobs[1]
                .args
                .windows(2)
                .any(|w| w[0] == "--resume" && w[1] == "s-1")
        );
        assert!(
            jobs[3]
                .stdin
                .as_deref()
                .unwrap()
                .contains("[HIGH] a.ts:1 — Null org")
        );
        assert!(
            jobs[4]
                .args
                .windows(2)
                .any(|w| w[0] == "--sandbox" && w[1] == "read-only")
        );
        assert!(jobs[6].stdin.as_deref().unwrap().contains("(#279)"));

        let d = run(&flow, &ctx, FlowAction::Finish { forced: false });
        assert_eq!(d.status, FlowStatus::Done);
        assert!(flow.is_finished(1));
    }

    #[test]
    fn finished_tasks_carry_on_or_run_again_from_a_step() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, happy(CLEAN));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();

        // done at the plan gate: the steps left are skipped; "Làm tiếp" comes back to the gate
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.status, FlowStatus::Gate, "{:?}", d.error);
        let d = run(&flow, &ctx, FlowAction::Finish { forced: false });
        assert_eq!(d.status, FlowStatus::Done);
        assert!(d.log.last().unwrap().text.contains("bỏ qua các bước còn lại"));
        let card = flow.card(&ctx).unwrap();
        assert_eq!(card.column, TaskColumn::Done);
        assert!(card.finished_at.is_some());
        let d = run(
            &flow,
            &ctx,
            FlowAction::Reopen {
                step: None,
                note: None,
            },
        );
        assert_eq!(
            (d.status, d.gate.as_ref().map(|g| g.kind)),
            (FlowStatus::Gate, Some(GateKind::PlanReview))
        );
        assert!(!flow.is_finished(1));
        let card = flow.card(&ctx).unwrap();
        assert_eq!((card.column, card.finished_at), (TaskColumn::Active, None));

        // through to the push summary and done
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        let d = to_push(&flow, &ctx, d);
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);
        run(&flow, &ctx, FlowAction::Finish { forced: true });
        // Nhận task cannot run again, and a refused "Làm tiếp" leaves the task done
        let not_reached = flow
            .act(
                ctx.clone(),
                FlowAction::Reopen {
                    step: Some(StepKind::Intake),
                    note: None,
                },
            )
            .unwrap_err();
        assert_eq!(not_reached.code, "flow.invalid", "{not_reached:?}");
        assert!(flow.is_finished(1));

        // back to Viết code with a note: earlier steps stay done, the rest start over
        let d = run(
            &flow,
            &ctx,
            FlowAction::Reopen {
                step: Some(StepKind::Implement),
                note: Some(" Thêm test cho org rỗng ".into()),
            },
        );
        assert_eq!(
            (d.status, d.current),
            (FlowStatus::Ready, Some(StepKind::Implement))
        );
        let state = |d: &TaskDetail, k: StepKind| d.steps.iter().find(|s| s.kind == k).unwrap().state;
        assert_eq!(state(&d, StepKind::Analyze), StepState::Done);
        assert_eq!(state(&d, StepKind::PushSummary), StepState::Pending);
        assert_eq!(d.round, 0);
        assert!(
            d.log
                .last()
                .unwrap()
                .text
                .contains("chạy lại từ Viết code, kèm ghi chú: Thêm test cho org rỗng")
        );
        assert_eq!(flow.card(&ctx).unwrap().column, TaskColumn::Active);

        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.current, Some(StepKind::ReviewLocal), "{:?}", d.error);
        let last = fake.jobs.lock().unwrap().last().cloned().unwrap();
        assert_eq!(kind(&last), "code");
        assert!(
            last.stdin
                .as_deref()
                .unwrap()
                .contains("<user_note>\nThêm test cho org rỗng\n</user_note>")
        );
        // the note goes to one Viết code only
        run(
            &flow,
            &ctx,
            FlowAction::Reopen {
                step: Some(StepKind::Implement),
                note: None,
            },
        );
        run(&flow, &ctx, FlowAction::Run);
        let last = fake.jobs.lock().unwrap().last().cloned().unwrap();
        assert!(!last.stdin.as_deref().unwrap().contains("user_note"));
    }

    #[test]
    fn starting_a_finished_task_again_keeps_its_history() {
        let Some(s) = setup() else { return };
        let (flow, _) = flow_with(&s, false, happy(CLEAN));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        let d = to_push(&flow, &ctx, d);
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);
        run(&flow, &ctx, FlowAction::Finish { forced: false });
        let runs_before = flow.detail(&ctx).runs.len();
        assert!(runs_before >= 4);

        // dragged to the Inbox and started again, back to Viết code with a note
        flow.started(
            &ctx,
            Some(&CarryOn {
                step: Some(StepKind::Implement),
                note: Some("khách mở lại".into()),
            }),
        )
        .unwrap();
        let d = flow.detail(&ctx);
        assert_eq!(
            (d.status, d.current),
            (FlowStatus::Ready, Some(StepKind::Implement))
        );
        let state = |k: StepKind| d.steps.iter().find(|s| s.kind == k).unwrap().state;
        assert_eq!(state(StepKind::Analyze), StepState::Done);
        assert_eq!(d.runs.len(), runs_before);
        assert!(d.plan.is_some());
        assert!(!flow.is_finished(1));

        // a paused task started again keeps its place too
        flow.started(&ctx, None).unwrap();
        assert_eq!(flow.detail(&ctx).current, Some(StepKind::Implement));
    }

    /// "Tự chạy" (one switch per task) runs step after step, but stops at every decision and
    /// says so: plan, each review's findings, "Review chéo lại?" after each fix, push.
    #[test]
    fn auto_advance_stops_at_every_decision_and_announces_it() {
        let Some(s) = setup() else { return };
        let (flow, _) = flow_with(&s, false, happy(MEDIUM));
        let alerts = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = alerts.clone();
        flow.set_alert_sink(move |a| sink.lock().unwrap().push(a.title));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        // the switch is per task
        let d = run(&flow, &ctx, FlowAction::SetAuto { on: true });
        assert!(d.auto_advance);
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.gate.map(|g| g.kind), Some(GateKind::PlanReview));
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        // implement ran by itself, then Tự review: its high waits for the user
        let gate = d.gate.clone().unwrap();
        assert_eq!(
            (gate.kind, gate.review),
            (GateKind::Findings, Some(StepKind::ReviewLocal)),
            "{:?}",
            d.error
        );
        pick_all(&flow, &ctx);
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::AfterFix),
            "{:?}",
            d.error
        );
        // Codex is clean, the final review reports a medium: its gate leads to the push summary
        let d = run(
            &flow,
            &ctx,
            FlowAction::AfterFix {
                choice: AfterFix::CrossReview,
            },
        );
        let gate = d.gate.clone().unwrap();
        assert_eq!(
            (gate.kind, gate.review, gate.next, d.round),
            (
                GateKind::Findings,
                Some(StepKind::FinalReview),
                Some(StepKind::PushSummary),
                1
            ),
            "{:?}",
            d.error
        );
        assert!(
            gate.message.ends_with("Đã review chéo 1 vòng."),
            "{}",
            gate.message
        );
        let medium = d
            .findings
            .iter()
            .find(|f| f.severity == Severity::Medium)
            .unwrap()
            .fingerprint
            .clone();
        let decide = |decision, reason: Option<&str>| {
            flow.decide(&FindingDecision {
                wp_id: 1,
                fingerprint: medium.clone(),
                decision,
                reason: reason.map(str::to_owned),
            })
        };
        assert_eq!(decide(Decision::Accept, None).unwrap_err().code, "finding.reason");
        decide(Decision::Accept, Some("chỉ là test")).unwrap();
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: false });
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);
        let accepted = d.findings.iter().find(|f| f.fingerprint == medium).unwrap();
        assert_eq!(accepted.reason.as_deref(), Some("chỉ là test"));
        assert!(
            d.log
                .iter()
                .any(|l| l.text == "Bạn chọn Review chéo lại (vòng 1)")
        );

        // one alert per stop, none between steps that ran by themselves (the last one is sent
        // just after the step ends)
        let deadline = Instant::now() + Duration::from_secs(5);
        while alerts.lock().unwrap().len() < 5 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            *alerts.lock().unwrap(),
            [
                "WP-1 · plan chờ bạn duyệt",
                "WP-1 · Tự review báo lỗi: chọn lỗi cần sửa",
                "WP-1 · sửa lỗi xong: Review chéo lại?",
                "WP-1 · Review chốt báo lỗi: chọn lỗi cần sửa",
                "WP-1 · sẵn sàng push",
            ]
        );
        // the same medium reported again later is not asked about again
        let (_, note) = review_event(
            &flow.inner.findings(1),
            &[RawFinding {
                severity: "medium".into(),
                file: "a.ts".into(),
                line: Some(1),
                title: "Missing test".into(),
                detail: String::new(),
                suggestion: String::new(),
            }],
        );
        assert_eq!(
            note,
            "báo 1 lỗi (1 medium), 1 lỗi bạn đã chấp nhận / để sau từ trước"
        );
    }

    /// Without "Tự chạy" the user can also skip the cross review after a fix, or pick more.
    #[test]
    fn after_a_fix_the_user_can_pick_more_or_go_on_without_codex() {
        let Some(s) = setup() else { return };
        let two = "```json\n{\"findings\":[{\"severity\":\"high\",\"file\":\"a.ts\",\"line\":1,\"title\":\"Null org\"},{\"severity\":\"low\",\"file\":\"a.ts\",\"line\":9,\"title\":\"Name\"}]}\n```";
        let reviews = AtomicUsize::new(0);
        let inner = happy(CLEAN);
        let (flow, fake) = flow_with(
            &s,
            false,
            move |job: &Job, stop: &AtomicBool, out: &mut dyn FnMut(&str)| {
                if kind(job) == "review" {
                    let first = reviews.fetch_add(1, Ordering::SeqCst) == 0;
                    out(&result_line(if first { two } else { CLEAN }));
                    return ok();
                }
                inner(job, stop, out)
            },
        );
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        run(&flow, &ctx, FlowAction::ApprovePlan);
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.gate.as_ref().map(|g| g.kind), Some(GateKind::Findings));
        // fix only the high
        let high = d.findings.iter().find(|f| f.severity == Severity::High).unwrap();
        flow.decide(&FindingDecision {
            wp_id: 1,
            fingerprint: high.fingerprint.clone(),
            decision: Decision::Fix,
            reason: None,
        })
        .unwrap();
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        assert_eq!(d.gate.as_ref().map(|g| g.kind), Some(GateKind::AfterFix));
        // back to Tự review's findings: the low is still there, the high is not taken twice
        let d = run(
            &flow,
            &ctx,
            FlowAction::AfterFix {
                choice: AfterFix::PickMore,
            },
        );
        let gate = d.gate.clone().unwrap();
        assert_eq!(
            (gate.kind, gate.review),
            (GateKind::Findings, Some(StepKind::ReviewLocal))
        );
        assert!(
            gate.message.contains("1 lỗi đang mở, đã chọn 0"),
            "{}",
            gate.message
        );
        pick_all(&flow, &ctx);
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        assert_eq!(d.gate.as_ref().map(|g| g.kind), Some(GateKind::AfterFix));
        let fixes: Vec<String> = fake
            .jobs
            .lock()
            .unwrap()
            .iter()
            .filter(|j| kind(j) == "code")
            .skip(1)
            .map(|j| j.stdin.clone().unwrap())
            .collect();
        assert_eq!(fixes.len(), 2);
        assert!(fixes[0].contains("Null org") && !fixes[0].contains("Name"));
        assert!(fixes[1].contains("Name") && !fixes[1].contains("Null org"));

        // no cross review: Review chéo and step 9 are "bỏ qua", the final review runs now
        let d = run(
            &flow,
            &ctx,
            FlowAction::AfterFix {
                choice: AfterFix::Next,
            },
        );
        assert_eq!(
            (d.status, d.current, d.round),
            (FlowStatus::Ready, Some(StepKind::PushSummary), 0),
            "{:?}",
            d.error
        );
        let state = |k: StepKind| d.steps.iter().find(|s| s.kind == k).unwrap().state;
        assert_eq!(state(StepKind::CrossReview), StepState::Skipped);
        assert_eq!(state(StepKind::FixCross), StepState::Skipped);
        assert_eq!(state(StepKind::Fix), StepState::Done);
        assert_eq!(state(StepKind::FinalReview), StepState::Done);
        let jobs = fake.jobs.lock().unwrap().clone();
        assert!(!jobs.iter().any(|j| kind(j) == "codex"));
        assert_eq!(jobs.iter().filter(|j| kind(j) == "review").count(), 2);
        // the final review was clean: what was being fixed is fixed
        assert!(
            d.findings
                .iter()
                .all(|f| f.state == opw_domain::finding::FindingState::Fixed)
        );
        // answers only count at their own gate
        assert_eq!(
            flow.act(
                ctx.clone(),
                FlowAction::AfterFix {
                    choice: AfterFix::Next
                }
            )
            .unwrap_err()
            .code,
            "flow.notNow"
        );
    }

    #[test]
    fn a_read_only_step_that_changes_files_or_a_commit_stops_the_flow() {
        let Some(s) = setup() else { return };
        let (flow, _) = flow_with(&s, false, |job, _, out| {
            if kind(job) == "analyze" {
                std::fs::write(job.cwd.join("b.ts"), "x").unwrap();
                out(&result_line("## Nguyên nhân\nx"));
            }
            ok()
        });
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.error.as_ref().map(|e| e.code.as_str()), Some("git.unexpected"));
        assert_eq!(d.gate.map(|g| g.kind), Some(GateKind::Error));
        assert_eq!(flow.card(&ctx).unwrap().step.state, StepState::Failed);

        std::fs::remove_file(s.repo.join("b.ts")).unwrap();
        let (flow, _) = flow_with(&s, false, |job, _, out| {
            git(&job.cwd, &["commit", "-q", "--allow-empty", "-m", "sneaky"]);
            out(&result_line("## Nguyên nhân\nx"));
            ok()
        });
        let d = run(&flow, &ctx, FlowAction::Retry);
        let err = d.error.unwrap();
        assert_eq!(err.code, "git.unexpected");
        assert!(err.message.contains("HEAD"), "{}", err.message);
    }

    #[test]
    fn signed_out_and_missing_clis_say_what_to_do() {
        let Some(s) = setup() else { return };
        let (flow, _) = flow_with(&s, false, |_, _, out| {
            out(
                r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#,
            );
            RunOutcome {
                code: Some(1),
                ..RunOutcome::default()
            }
        });
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let err = run(&flow, &ctx, FlowAction::Run).error.unwrap();
        assert_eq!(
            (err.code.as_str(), err.provider),
            ("agent.signedOut", Some(Provider::Claude))
        );
        assert_eq!(flow.card(&ctx).unwrap().login, Some(Provider::Claude));

        let (flow, _) = flow_with(&s, true, |_, _, _| ok());
        let err = run(&flow, &ctx, FlowAction::Retry).error.unwrap();
        assert_eq!(err.code, "agent.missing");
    }

    #[test]
    fn one_step_per_repo_at_a_time_and_stop_works() {
        let Some(s) = setup() else { return };
        let (flow, _) = flow_with(&s, false, |_, stop, _| {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            RunOutcome {
                stopped: true,
                ..RunOutcome::default()
            }
        });
        let one = s.ctx(1, &Config::default());
        let two = s.ctx(2, &Config::default());
        flow.started(&one, None).unwrap();
        flow.started(&two, None).unwrap();
        flow.act(one.clone(), FlowAction::Run).unwrap();
        assert_eq!(flow.detail(&one).status, FlowStatus::Running);
        assert_eq!(
            flow.act(two.clone(), FlowAction::Run).unwrap_err().code,
            "repo.busy"
        );
        assert_eq!(
            flow.act(one.clone(), FlowAction::Run).unwrap_err().code,
            "step.running"
        );
        // a running step has to be stopped before the task is marked done or sent back
        for action in [
            FlowAction::Finish { forced: true },
            FlowAction::Reopen {
                step: Some(StepKind::Analyze),
                note: None,
            },
        ] {
            assert_eq!(flow.act(one.clone(), action).unwrap_err().code, "step.running");
        }
        assert_eq!(flow.running_step(1), Some(StepKind::Analyze));
        flow.act(one.clone(), FlowAction::Stop).unwrap();
        let d = settle(&flow, &one);
        assert_eq!(d.error.unwrap().code, "step.stopped");
        assert_eq!(d.current, Some(StepKind::Analyze));
        assert_eq!(flow.running_step(1), None);
    }

    #[test]
    fn a_step_running_when_the_app_closed_can_be_retried() {
        let Some(s) = setup() else { return };
        let ctx = s.ctx(1, &Config::default());
        let mut saved = new_saved();
        saved.in_flight = true;
        s.store
            .lock()
            .unwrap()
            .save_flow(1, &serde_json::to_string(&saved).unwrap(), "2026-11-14T08:00:00Z")
            .unwrap();
        let (flow, _) = flow_with(&s, false, |_, _, _| ok());
        let d = flow.detail(&ctx);
        assert_eq!(d.error.unwrap().code, "step.interrupted");
        assert_eq!(d.gate.map(|g| g.kind), Some(GateKind::Error));
        assert_eq!(d.current, Some(StepKind::Analyze));
    }

    #[test]
    fn self_code_skips_implement_and_reviews_the_users_change() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, happy(CLEAN));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        assert_eq!(
            flow.act(ctx.clone(), FlowAction::RejectPlan { feedback: " ".into() })
                .unwrap_err()
                .code,
            "plan.feedback"
        );
        let d = run(
            &flow,
            &ctx,
            FlowAction::RejectPlan {
                feedback: "Dùng phương án 2".into(),
            },
        );
        assert_eq!(d.gate.map(|g| g.kind), Some(GateKind::PlanReview));
        let again = fake.jobs.lock().unwrap()[1].stdin.clone().unwrap();
        assert!(again.contains("<feedback>\nDùng phương án 2\n</feedback>"));
        let d = run(&flow, &ctx, FlowAction::SelfCode);
        assert_eq!(d.gate.map(|g| g.kind), Some(GateKind::SelfCode));
        let d = run(&flow, &ctx, FlowAction::SelfCodeDone);
        // Tự review ran on the user's change and found the high
        assert_eq!(
            (d.current, d.gate.map(|g| g.kind)),
            (Some(StepKind::ReviewLocal), Some(GateKind::Findings)),
            "{:?}",
            d.error
        );
        assert_eq!(
            fake.jobs
                .lock()
                .unwrap()
                .iter()
                .filter(|j| kind(j) == "code")
                .count(),
            0
        );
    }

    #[test]
    fn picked_models_reach_the_cli_and_review_notes_are_kept() {
        let Some(s) = setup() else { return };
        let notes = "```json\n{\"findings\":[],\"notes\":\"Cần thêm test cho org rỗng\"}\n```";
        let (flow, fake) = flow_with(&s, false, happy(notes));
        let mut config = Config::default();
        config.steps.auto_advance = true;
        config.steps.implement.model = Some("claude-sonnet-5-5".into());
        config.steps.implement.effort = Some("high".into());
        config.steps.cross_review.model = Some("gpt-x".into());
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        // nothing picked for analyze: no flag, and the model Claude reported is shown
        assert_eq!(d.steps[1].model.as_deref(), Some("m"));
        assert_eq!(d.steps[1].effort, None);
        run(&flow, &ctx, FlowAction::ApprovePlan);
        pick_all(&flow, &ctx);
        run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        let d = run(
            &flow,
            &ctx,
            FlowAction::AfterFix {
                choice: AfterFix::CrossReview,
            },
        );
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);

        let jobs = fake.jobs.lock().unwrap().clone();
        let flag = |job: &Job, name: &str| job.args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
        assert_eq!(flag(&jobs[0], "--model"), None);
        assert_eq!(flag(&jobs[1], "--model").as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(flag(&jobs[1], "--effort").as_deref(), Some("high"));
        assert_eq!(flag(&jobs[2], "--model"), None);
        assert_eq!(flag(&jobs[4], "-m").as_deref(), Some("gpt-x"));
        let push = jobs.last().unwrap();
        assert_eq!(flag(push, "--effort").as_deref(), Some("low"));

        let implement = &d.steps[StepKind::Implement.index() as usize - 1];
        assert_eq!(implement.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(implement.effort.as_deref(), Some("high"));
        assert_eq!(
            d.review_notes,
            [ReviewNote {
                step: StepKind::FinalReview,
                text: "Cần thêm test cho org rỗng".into()
            }]
        );
        assert!(
            d.log
                .iter()
                .any(|l| l.text == "Bắt đầu Tự review (Claude · prompt review của app)"),
            "{:?}",
            d.log
        );
        assert_eq!(step_label(StepKind::PushSummary), "Push summary");
    }

    /// Analyze writes a plan with two files and a question; the code step edits a.ts and has one
    /// chained command refused; reviews are clean; Codex answers with a thread id.
    fn with_denial() -> impl ScriptFn {
        move |job, _stop, out| {
            match kind(job) {
                "analyze" => {
                    out(&init_line());
                    out(&result_line(
                        "## Nguyên nhân\nx\n## File sẽ sửa\n- `a.ts`\n  - chi tiết\n- `b.ts`\n## Rủi ro và câu hỏi\n- Có cần migrate không?\n- Rủi ro thấp.",
                    ));
                }
                "code" => {
                    let file = job.cwd.join("a.ts");
                    let old = std::fs::read_to_string(&file).unwrap();
                    std::fs::write(&file, format!("{old}// edit\n")).unwrap();
                    out(
                        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Chạy type check."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cd api && npx tsc --noEmit"}}]}}"#,
                    );
                    out(
                        r#"{"type":"system","subtype":"permission_denied","tool_use_id":"t1","decision_reason":"not allowed"}"#,
                    );
                    let mut result: serde_json::Value =
                        serde_json::from_str(&result_line("đã sửa a.ts")).unwrap();
                    result["permission_denials"] = serde_json::json!([
                        {"tool_name": "Bash", "tool_input": {"command": "cd api && npx tsc --noEmit"}}
                    ]);
                    out(&result.to_string());
                }
                "review" => out(&result_line(CLEAN)),
                "codex" => {
                    out(r#"{"type":"thread.started","thread_id":"th-9"}"#);
                    out(r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}"#);
                    let i = job.args.iter().position(|a| a == "-o").unwrap();
                    std::fs::write(&job.args[i + 1], "{\"findings\":[]}").unwrap();
                }
                _ => out(&result_line(PUSH)),
            }
            ok()
        }
    }

    #[test]
    fn runs_keep_a_note_a_chat_and_the_refused_commands() {
        let Some(s) = setup() else { return };
        let fake = Arc::new(Fake {
            script: Box::new(with_denial()),
            jobs: Mutex::new(Vec::new()),
            missing: false,
        });
        let runs_dir = s.tasks.with_file_name("runs");
        let flow = Flow::new(s.store.clone(), fake.clone(), Some(runs_dir.clone()));
        let mut config = Config::default();
        config.repos.insert(
            s.repo.to_string_lossy().into_owned(),
            RepoConfig {
                allow_bash: vec!["Bash(npx tsc *)".into()],
                ..RepoConfig::default()
            },
        );
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        assert!(flow.detail(&ctx).runs.is_empty());
        assert_eq!(flow.chat(&ctx, None).unwrap_err().code, "run.missing");

        run(&flow, &ctx, FlowAction::Run);
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(d.current, Some(StepKind::ReviewLocal), "{:?}", d.error);
        let [analyze, implement] = &d.runs[..] else {
            panic!("{:?}", d.runs)
        };
        assert_eq!((analyze.step, analyze.state), (StepKind::Analyze, RunState::Done));
        assert_eq!(analyze.note.as_deref(), Some("plan: 2 file sẽ sửa, 1 câu hỏi"));
        assert!(analyze.has_chat);
        assert_eq!(analyze.resume.as_deref(), Some("claude --resume s-1"));
        assert_eq!(analyze.cost_usd, Some(0.25));
        assert_eq!(
            implement.note.as_deref(),
            Some("1 file thay đổi (+1 −0), 1 lệnh bị chặn")
        );
        let denied = &implement.denials[0];
        assert_eq!(denied.input, "cd api && npx tsc --noEmit");
        assert!(denied.chained);
        assert_eq!(denied.rules.len(), 1);
        assert_eq!(denied.rules[0].rule, "npx tsc *");
        assert!(denied.rules[0].allowed);
        assert!(
            d.log
                .iter()
                .any(|l| l.text == "Viết code xong: 1 file thay đổi (+1 −0), 1 lệnh bị chặn ($0.25)")
        );
        // the code prompt lists what may run and asks for one command at a time
        let jobs = fake.jobs.lock().unwrap().clone();
        let code = jobs[1].stdin.as_deref().unwrap();
        assert!(code.contains("allowed in this repository: `Bash(npx tsc *)`"));
        assert!(code.contains("Do not chain commands"));

        // the latest run's chat: the prompt first, the refused command marked
        let chat = flow.chat(&ctx, None).unwrap();
        assert_eq!(chat.run.id, implement.id);
        assert!(!chat.missing);
        assert_eq!(chat.items[0].kind, ChatKind::Prompt);
        assert!(chat.items[0].text.starts_with("Implement the approved plan"));
        let tool = chat.items.iter().find(|i| i.kind == ChatKind::Tool).unwrap();
        assert_eq!(tool.denied.as_deref(), Some("not allowed"));
        let first = flow.chat(&ctx, Some(analyze.id)).unwrap();
        assert!(first.items[0].text.contains("<brief>"));

        // the review is clean, then Codex keeps its thread for `codex resume`
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.runs[2].note.as_deref(), Some("không thấy lỗi"));
        let d = run(&flow, &ctx, FlowAction::Run);
        let cross = &d.runs[3];
        assert_eq!(cross.agent, AgentKind::Codex);
        assert_eq!(cross.note.as_deref(), Some("vòng 1: không thấy lỗi"));
        assert_eq!(cross.resume.as_deref(), Some("codex resume th-9"));
        let (repo, provider, session) = flow.resume_target(&ctx, cross.id).unwrap();
        assert_eq!((provider, session.as_str()), (Provider::Codex, "th-9"));
        assert_eq!(repo, s.repo);
        assert!(!jobs.iter().any(|j| j.args.contains(&"--ephemeral".to_owned())));

        // a log that is gone shows as missing, not as an error
        std::fs::remove_dir_all(&runs_dir).unwrap();
        let gone = flow.chat(&ctx, Some(analyze.id)).unwrap();
        assert!(gone.missing && gone.items.is_empty());
        assert!(!flow.detail(&ctx).runs[0].has_chat);
    }

    #[test]
    fn runs_from_before_migration_5_find_their_log_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("WP-1");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("2026-11-14T10-31-19-7913326Z-implement.jsonl"), "{}").unwrap();
        std::fs::write(folder.join("2026-11-14T10-37-37-268938Z-reviewLocal.jsonl"), "{}").unwrap();
        let mut row = StepRunRow {
            id: 1,
            wp_id: 1,
            step: "implement".into(),
            agent: Some("claude".into()),
            session_id: None,
            state: "done".into(),
            round: None,
            cost_usd: None,
            tokens_in: None,
            tokens_out: None,
            started_at: "2026-11-14T10:31:19.7801Z".into(),
            ended_at: None,
            model: None,
            effort: None,
            note: None,
            log_file: None,
            denials: None,
        };
        let found = log_path(&row, Some(tmp.path()), "WP-1").unwrap();
        assert!(found.ends_with("2026-11-14T10-31-19-7913326Z-implement.jsonl"));
        // another step, or a start far from the file, finds nothing
        row.started_at = "2026-11-14T09:00:00Z".into();
        assert!(log_path(&row, Some(tmp.path()), "WP-1").is_none());
        row.step = "fix".into();
        assert!(log_path(&row, Some(tmp.path()), "WP-1").is_none());
        assert!(log_path(&row, None, "WP-1").is_none());
    }

    #[test]
    fn resume_needs_a_plain_session_and_no_push_summary() {
        assert_eq!(
            resume_command(StepKind::Fix, AgentKind::Claude, Some("a-1")).as_deref(),
            Some("claude --resume a-1")
        );
        assert_eq!(
            resume_command(StepKind::PushSummary, AgentKind::Claude, Some("a-1")),
            None
        );
        assert_eq!(
            resume_command(StepKind::Fix, AgentKind::Claude, Some("a & calc")),
            None
        );
        assert_eq!(resume_command(StepKind::Fix, AgentKind::Claude, None), None);
        assert_eq!(plan_note("## Nguyên nhân\nkhông có danh sách"), "đã viết plan.md");
        let job = Job {
            agent: Provider::Claude,
            args: claude::review_local_args("main", 1.0),
            cwd: PathBuf::new(),
            stdin: None,
            timeout: Duration::from_secs(1),
        };
        let prompt = prompt_of(&job);
        assert!(prompt.starts_with("/review-local\n\nNon-interactive run."));
    }

    #[test]
    fn final_review_limits_fall_back_to_review_local() {
        let mut config = Config::default();
        config.steps.review_local.max_budget_usd = Some(4.0);
        assert_eq!(limits(StepKind::FinalReview, &config).0, 4.0);
        config.steps.final_review.max_budget_usd = Some(2.0);
        assert_eq!(
            limits(StepKind::FinalReview, &config),
            (2.0, Duration::from_secs(1800), "finalReview")
        );
        assert_eq!(
            model_choice(StepKind::PushSummary, &config).effort.as_deref(),
            Some("low")
        );
        config.steps.push_summary.effort = Some("medium".into());
        assert_eq!(
            model_choice(StepKind::PushSummary, &config).effort.as_deref(),
            Some("medium")
        );
        assert_eq!(model_choice(StepKind::Analyze, &config), ModelChoice::default());
    }

    #[test]
    fn push_blocks_fall_back_to_the_whole_text() {
        let v = push_view("just text", None);
        assert_eq!(v.blocks.len(), 1);
        assert_eq!(v.blocks[0].label, "Nội dung");
        assert!(push_view("", None).blocks.is_empty());
    }
}
