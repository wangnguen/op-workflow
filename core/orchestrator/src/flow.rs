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
//!   result before the next step starts.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opw_agents::claude::{self, ClaudeEvent, RunResult};
use opw_agents::codex::{self, CodexEvent};
use opw_agents::env::Account;
use opw_agents::run::{RunOutcome, RunSpec};
use opw_agents::text::{self, RawFinding};
use opw_config::{Config, RepoConfig, StepConfig};
use opw_context::prompts::{self, FixItem};
use opw_domain::finding::{GateAction, GatePolicy, Severity, fingerprint};
use opw_domain::view::{
    AgentKind, AgentView, AlertKind, ApiError, Decision, FindingDecision, FindingView, FlowAction, FlowError,
    FlowStatus, GateKind, GateView, LogLine, Provider, PushBlock, PushView, QuotaSnapshot, RunView,
    SessionStatus, StepRef, StepView, TaskColumn, TaskDetail, ToolEvent,
};
use opw_domain::workflow::{Phase, StepKind, StepState, Workflow, WorkflowEvent};
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
    /// Fix runs so far (a cap stops endless review ⇄ fix).
    fixes: u32,
    /// Feedback on the plan for the next analyze.
    feedback: Option<String>,
    /// When the phase last changed.
    since: Option<String>,
    /// A process was running when this was saved.
    in_flight: bool,
    /// The user said the push is done.
    finished: bool,
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

    fn max_rounds(&self) -> u8 {
        self.config
            .steps
            .cross_review
            .max_rounds
            .unwrap_or(opw_domain::workflow::DEFAULT_MAX_REVIEW_ROUNDS)
    }

    fn agent_for(&self, step: StepKind) -> AgentKind {
        agent_for(step, &self.config)
    }
}

fn agent_for(step: StepKind, config: &Config) -> AgentKind {
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
        StepKind::ReviewLocal => "reviewLocal",
        StepKind::Fix => "fix",
        StepKind::CrossReview => "crossReview",
        StepKind::FinalReview => "finalReview",
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
        StepKind::ReviewLocal => "Tự review",
        StepKind::Fix => "Sửa lỗi",
        StepKind::CrossReview => "Review chéo",
        StepKind::FinalReview => "Review chốt",
        StepKind::PushSummary => "Chuẩn bị push",
    }
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
        StepKind::ReviewLocal | StepKind::FinalReview => (&steps.review_local, 3.0, 30, "reviewLocal"),
        StepKind::Fix => (&steps.fix, 2.0, 45, "fix"),
        StepKind::CrossReview => (&steps.cross_review, 3.0, 30, "crossReview"),
        StepKind::PushSummary => (&steps.push_summary, 1.0, 10, "pushSummary"),
        StepKind::Intake => (&steps.analyze, 0.0, 1, "intake"),
    };
    let budget = cfg.max_budget_usd.filter(|b| *b > 0.0).unwrap_or(budget);
    let minutes = cfg.timeout_min.filter(|m| *m > 0).unwrap_or(minutes);
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

/// Unsettled and either blocking/asked by the gate policy, or picked by the user.
fn to_fix(f: &FindingRow, policy: &GatePolicy) -> bool {
    !settled(&f.state) && (f.chosen || policy.action(severity_of(&f.severity)) != GateAction::Info)
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
    !matches!(step, StepKind::Implement | StepKind::Fix)
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
}

/// What a step produced: the event for the state machine and a line for the log.
struct Done {
    event: WorkflowEvent,
    note: String,
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
    pub fn started(&self, ctx: &TaskCtx) {
        let mut tasks = self.inner.lock_tasks();
        let entry = tasks.entry(ctx.row.wp_id).or_default();
        if entry.saved.finished || entry.saved.workflow.phase() == Phase::NotStarted {
            entry.saved = new_saved(ctx);
            push_log(entry, None, "info", "Đã nhận task: tạo nhánh và brief xong");
            self.inner.persist(ctx.row.wp_id, entry);
        }
    }

    /// Make sure a task in progress has a workflow (tasks started before steps 2-8 existed).
    pub fn ensure(&self, ctx: &TaskCtx) {
        let mut tasks = self.inner.lock_tasks();
        if !tasks.contains_key(&ctx.row.wp_id) {
            let entry = tasks.entry(ctx.row.wp_id).or_default();
            entry.saved = new_saved(ctx);
            self.inner.persist(ctx.row.wp_id, entry);
        }
    }

    pub fn is_finished(&self, wp_id: u64) -> bool {
        self.inner
            .lock_tasks()
            .get(&wp_id)
            .is_some_and(|e| e.saved.finished)
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
                gate.map(|g| gate_message(g, entry, &self.inner.findings(ctx.row.wp_id), &ctx.config))
            }
            _ => None,
        };
        let cost: f64 = runs.iter().filter_map(|r| r.cost_usd).sum();
        Some(CardFlow {
            column: if matches!(status, FlowStatus::AwaitingPush | FlowStatus::Done) {
                TaskColumn::AwaitingPush
            } else {
                TaskColumn::Active
            },
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
            gate: gate_view(entry, &findings, &ctx.config),
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
            repo: ctx.row.repo.clone(),
            branch: ctx.row.branch.clone(),
            base: ctx.row.base.clone(),
            status: status_of(entry),
            current: current_step(entry),
            steps: step_views(entry, &runs, &ctx.row.updated_at),
            gate: gate_view(entry, &findings, &ctx.config),
            error: entry.saved.error.clone(),
            running: entry.run.as_ref().map(|r| RunView {
                step: r.step,
                agent: r.agent,
                started_at: r.started_at.clone(),
                tools: r.tools.iter().cloned().collect(),
                context_tokens: r.context,
            }),
            round: entry.saved.workflow.round(),
            max_rounds: entry.saved.workflow.max_rounds(),
            auto_advance: ctx.auto(&entry.saved),
            brief: ctx.read(BRIEF_FILE),
            plan: ctx.read(PLAN_FILE),
            findings: findings.iter().map(finding_view).collect(),
            push,
            log: entry.log.iter().cloned().collect(),
            cost_usd: runs.iter().filter_map(|r| r.cost_usd).sum(),
            // the facade fills this from OpenProject and the last issue read
            new_comments: 0,
        }
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

fn new_saved(ctx: &TaskCtx) -> Saved {
    let mut workflow = Workflow::new(ctx.max_rounds());
    let _ = workflow.apply(WorkflowEvent::Start);
    let _ = workflow.apply(WorkflowEvent::StepSucceeded(StepKind::Intake));
    Saved {
        workflow,
        since: Some(now()),
        ..Saved::default()
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
        FlowAction::FinishTriage => {
            let policy = &ctx.config.gate;
            let findings = store
                .lock()
                .ok()
                .and_then(|s| s.findings(ctx.row.wp_id).ok())
                .unwrap_or_default();
            let undecided = findings
                .iter()
                .filter(|f| {
                    !settled(&f.state)
                        && !f.chosen
                        && policy.action(severity_of(&f.severity)) == GateAction::Ask
                })
                .count();
            if undecided > 0 {
                return Err(ApiError::new(
                    "triage.open",
                    format!("Còn {undecided} lỗi medium chưa quyết (sửa, chấp nhận hoặc để sau)"),
                ));
            }
            let fix = findings.iter().filter(|f| to_fix(f, policy)).count();
            event(entry, E::MediumTriaged { fix })?;
            Ok(true)
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
        FlowAction::Finish => {
            if phase != Phase::AwaitingPush {
                return Err(not_now("đánh dấu xong"));
            }
            entry.saved.finished = true;
            entry.saved.since = Some(now());
            push_log(entry, None, "ok", "Bạn đã push: task xong");
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
            "log",
        ),
        Phase::Gate(GateKind::PlanReview) => (
            AlertKind::Gate,
            title_of("plan chờ bạn duyệt"),
            ctx.row.title.clone(),
            "plan",
        ),
        Phase::Gate(GateKind::MediumTriage) => (
            AlertKind::Gate,
            title_of("lỗi medium chờ bạn quyết"),
            ctx.row.title.clone(),
            "findings",
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
                "log",
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
            }
        })
        .collect()
}

fn gate_message(kind: GateKind, entry: &Entry, findings: &[FindingRow], config: &Config) -> String {
    match kind {
        GateKind::PlanReview => {
            "Plan đã xong: duyệt để AI viết code, góp ý để phân tích lại, hoặc tự code.".into()
        }
        GateKind::MediumTriage => {
            let n = findings
                .iter()
                .filter(|f| {
                    !settled(&f.state)
                        && !f.chosen
                        && config.gate.action(severity_of(&f.severity)) == GateAction::Ask
                })
                .count();
            format!("{n} lỗi medium cần bạn quyết: sửa, chấp nhận kèm lý do, hoặc để sau.")
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

fn gate_view(entry: &Entry, findings: &[FindingRow], config: &Config) -> Option<GateView> {
    if entry.run.is_some() {
        return None;
    }
    match entry.saved.workflow.phase() {
        Phase::Gate(kind) => Some(GateView {
            kind,
            since: entry.saved.since.clone().unwrap_or_default(),
            message: gate_message(kind, entry, findings, config),
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
        });
        entry.saved.in_flight = true;
        entry.saved.error = None;
        let text = format!("Bắt đầu {} ({})", step_label(step), agent_name(agent));
        push_log(entry, Some(step), "info", &text);
        self.persist(ctx.row.wp_id, entry);
        drop(tasks);
        if let Ok(store) = self.store.lock() {
            let _ = store.log_event(&now(), Some(ctx.row.wp_id), "flow.info", Some(&text));
        }
        Ok(Claim { step, agent, stop })
    }

    /// Record how a step ended and move the state machine. Returns `true` when the next step
    /// should run right away (auto-advance).
    fn finish(&self, ctx: &TaskCtx, step: StepKind, result: Result<Done, FlowError>) -> bool {
        let wp_id = ctx.row.wp_id;
        let mut tasks = self.lock_tasks();
        let Some(entry) = tasks.get_mut(&wp_id) else {
            return false;
        };
        entry.run = None;
        entry.saved.in_flight = false;
        entry.saved.since = Some(now());
        let (kind, text) = match result {
            Ok(done) => match entry.saved.workflow.apply(done.event) {
                Ok(()) => {
                    entry.saved.error = None;
                    if step == StepKind::Analyze {
                        entry.saved.feedback = None;
                    }
                    ("ok", format!("{} xong: {}", step_label(step), done.note))
                }
                Err(e) => {
                    let err = flow_error("flow.invalid", e.to_string());
                    let _ = entry.saved.workflow.apply(WorkflowEvent::StepFailed(step));
                    let text = err.message.clone();
                    entry.saved.error = Some(err);
                    ("error", text)
                }
            },
            Err(err) => {
                let _ = entry.saved.workflow.apply(WorkflowEvent::StepFailed(step));
                let text = format!("{} lỗi: {}", step_label(step), err.message);
                entry.saved.error = Some(err);
                if step == StepKind::Fix {
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
        drop(tasks);
        if let Some(alert) = alert {
            let sink = self.alert_sink.lock().ok().and_then(|s| s.clone());
            if let Some(sink) = sink {
                sink(alert);
            }
        }
        if let Ok(store) = self.store.lock() {
            let _ = store.log_event(&now(), Some(wp_id), &format!("flow.{kind}"), Some(&text));
        }
        go_on
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

    fn raw_log(&self, ctx: &TaskCtx, step: StepKind) -> Option<std::fs::File> {
        let dir = self.runs_dir.as_ref()?.join(&ctx.row.display_id);
        std::fs::create_dir_all(&dir).ok()?;
        let stamp = now().replace([':', '.'], "-");
        std::fs::File::create(dir.join(format!("{stamp}-{}.jsonl", step_key(step)))).ok()
    }

    /// Run Claude for one step; the final `result` line comes back.
    fn run_claude(
        &self,
        ctx: &TaskCtx,
        step: StepKind,
        job: &Job,
        stop: &AtomicBool,
    ) -> Result<RunResult, FlowError> {
        use std::io::Write;
        let wp_id = ctx.row.wp_id;
        let (budget, timeout, key) = limits(step, &ctx.config);
        let round = (step == StepKind::CrossReview).then(|| self.round_of(wp_id));
        let run_id = self.store.lock().ok().and_then(|s| {
            s.begin_step_run(wp_id, step_key(step), "claude", round, &now())
                .ok()
        });
        let mut raw = self.raw_log(ctx, step);
        let mut result: Option<RunResult> = None;
        let mut session = String::new();
        let mut on_line = |line: &str| {
            if let Some(f) = raw.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            for event in claude::parse_line(line).unwrap_or_default() {
                match event {
                    ClaudeEvent::Init { session_id, .. } => session = session_id,
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
                },
            );
        }
        if let Some(r) = &result {
            for d in &r.denials {
                self.log(
                    wp_id,
                    Some(step),
                    "info",
                    &format!("Bị từ chối (ngoài allowlist): {d}"),
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
        stop: &AtomicBool,
    ) -> Result<String, FlowError> {
        use std::io::Write;
        let wp_id = ctx.row.wp_id;
        let (_, timeout, key) = limits(step, &ctx.config);
        let round = (step == StepKind::CrossReview).then(|| self.round_of(wp_id));
        let run_id = self.store.lock().ok().and_then(|s| {
            s.begin_step_run(wp_id, step_key(step), "codex", round, &now())
                .ok()
        });
        let mut raw = self.raw_log(ctx, step);
        let mut last = String::new();
        let mut failed: Option<String> = None;
        let (mut tokens_in, mut tokens_out) = (0_u64, 0_u64);
        let mut on_line = |line: &str| {
            if let Some(f) = raw.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            match codex::parse_line(line) {
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
                    session_id: None,
                    cost_usd: None,
                    tokens_in: Some(tokens_in),
                    tokens_out: Some(tokens_out),
                    ended_at: &now(),
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

/// The state-machine event of a finished review, and a line for the log.
fn review_event(
    step: StepKind,
    findings: &[FindingRow],
    policy: &GatePolicy,
    reported: usize,
) -> (WorkflowEvent, String) {
    let open = findings.iter().filter(|f| !settled(&f.state)).count();
    let blocking = findings.iter().filter(|f| to_fix(f, policy)).count();
    let event = match step {
        StepKind::FinalReview => WorkflowEvent::FinalReviewFinished {
            critical_or_high: findings
                .iter()
                .filter(|f| {
                    !settled(&f.state)
                        && (f.chosen || policy.action(severity_of(&f.severity)) == GateAction::Block)
                })
                .count(),
            medium_undecided: findings
                .iter()
                .filter(|f| {
                    !settled(&f.state)
                        && !f.chosen
                        && policy.action(severity_of(&f.severity)) == GateAction::Ask
                })
                .count(),
        },
        StepKind::CrossReview => WorkflowEvent::CrossReviewFinished { blocking },
        _ => WorkflowEvent::ReviewLocalFinished { blocking },
    };
    (
        event,
        format!("báo {reported} lỗi; còn mở {open}, cần sửa {blocking}"),
    )
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
    let (session, feedback, fixes, issue) = inner
        .with_saved(wp_id, |s| {
            (s.session.clone(), s.feedback.clone(), s.fixes, s.issue)
        })
        .unwrap_or_default();
    let set_session = |id: &str| {
        if !id.is_empty() {
            inner.with_saved(wp_id, |s| s.session = Some(id.to_owned()));
        }
    };
    let policy = &ctx.config.gate;
    match step {
        StepKind::Intake => Ok(Done {
            event: E::StepSucceeded(StepKind::Intake),
            note: String::new(),
        }),
        StepKind::Analyze => {
            let previous_plan = ctx.read(PLAN_FILE);
            let previous = feedback.as_deref().zip(previous_plan.as_deref());
            let prompt = prompts::analyze(&brief, previous);
            let r = inner.run_claude(
                ctx,
                step,
                &job(Provider::Claude, claude::analyze_args(budget), Some(prompt)),
                &claim.stop,
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
                note: format!("plan.md, {}", usd(r.cost_usd)),
            })
        }
        StepKind::Implement => {
            let plan = ctx
                .read(PLAN_FILE)
                .ok_or_else(|| flow_error("plan.missing", "Không đọc được plan.md"))?;
            let prompt = prompts::implement(&plan, session.is_none().then_some(brief.as_str()));
            let args = claude::code_args(session.as_deref(), budget, &ctx.repo_config().allow_bash);
            let r = inner.run_claude(ctx, step, &job(Provider::Claude, args, Some(prompt)), &claim.stop)?;
            set_session(&r.session_id);
            Ok(Done {
                event: E::StepSucceeded(StepKind::Implement),
                note: usd(r.cost_usd),
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
            let r = inner.run_claude(ctx, step, &job(Provider::Claude, args, stdin), &claim.stop)?;
            let raw = text::findings_from_text(&r.text).ok_or_else(unreadable_review)?;
            let findings = inner.merge(ctx, step, 0, source, &raw)?;
            let (event, note) = review_event(step, &findings, policy, raw.len());
            Ok(Done {
                event,
                note: format!("{note}, {}", usd(r.cost_usd)),
            })
        }
        StepKind::Fix => {
            let cap = u32::from(ctx.max_rounds()) * 2 + 2;
            if fixes >= cap {
                return Err(flow_error(
                    "fix.cap",
                    format!(
                        "Đã sửa {fixes} lần mà review vẫn còn lỗi. App dừng để bạn xem lại tab Lỗi review."
                    ),
                ));
            }
            let findings = inner.findings(wp_id);
            let items: Vec<&FindingRow> = findings.iter().filter(|f| to_fix(f, policy)).collect();
            if items.is_empty() {
                return Ok(Done {
                    event: E::StepSucceeded(StepKind::Fix),
                    note: "không còn lỗi cần sửa".into(),
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
            let args = claude::code_args(session.as_deref(), budget, &ctx.repo_config().allow_bash);
            let r = inner.run_claude(
                ctx,
                step,
                &job(Provider::Claude, args, Some(prompts::fix(&list))),
                &claim.stop,
            )?;
            set_session(&r.session_id);
            inner.with_saved(wp_id, |s| s.fixes += 1);
            Ok(Done {
                event: E::StepSucceeded(StepKind::Fix),
                note: format!("{} lỗi, {}", list.len(), usd(r.cost_usd)),
            })
        }
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
                let args = codex::cross_review_args(&schema.to_string_lossy(), &out.to_string_lossy());
                let answer = inner.run_codex(
                    ctx,
                    step,
                    &job(Provider::Codex, args, Some(prompts::cross_review(&base, &brief))),
                    &claim.stop,
                )?;
                let text = std::fs::read_to_string(&out).unwrap_or(answer);
                let raw = serde_json::from_str::<serde_json::Value>(text.trim())
                    .ok()
                    .and_then(|v| text::findings_from_json(&v))
                    .or_else(|| text::findings_from_text(&text))
                    .ok_or_else(unreadable_review)?;
                (raw, "codex", None)
            } else {
                let stdin = Some(prompts::review(&base, &brief));
                let r = inner.run_claude(
                    ctx,
                    step,
                    &job(Provider::Claude, claude::review_args(budget), stdin),
                    &claim.stop,
                )?;
                let raw = text::findings_from_text(&r.text).ok_or_else(unreadable_review)?;
                (raw, "claude", Some(r.cost_usd))
            };
            let findings = inner.merge(ctx, step, round, source, &raw)?;
            let (event, note) = review_event(step, &findings, policy, raw.len());
            Ok(Done {
                event,
                note: match cost {
                    Some(c) => format!("vòng {round}: {note}, {}", usd(c)),
                    None => format!("vòng {round}: {note}"),
                },
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
            let r = inner.run_claude(ctx, step, &job(Provider::Claude, args, stdin), &claim.stop)?;
            if r.text.trim().is_empty() {
                return Err(flow_error(
                    "push.empty",
                    "Claude không trả về commit message. Xem Log rồi bấm Thử lại.",
                ));
            }
            write_task_file(ctx, PUSH_FILE, &r.text)?;
            Ok(Done {
                event: E::StepSucceeded(StepKind::PushSummary),
                note: format!("commit message + PR description, {}", usd(r.cost_usd)),
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
        if !inner.finish(ctx, step, result) {
            return;
        }
        match inner.claim(ctx) {
            Ok(next) => claim = next,
            Err(e) => {
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

    #[test]
    fn runs_all_steps_and_stops_at_each_one() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, happy(CLEAN));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx);
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

        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.current, Some(StepKind::Fix), "{:?}", d.error);
        assert_eq!(d.findings.len(), 1);
        assert_eq!(d.findings[0].severity, Severity::High);
        assert_eq!(d.findings[0].detail.as_deref(), Some("crash"));
        let card = flow.card(&ctx).unwrap();
        assert_eq!(
            (card.step.kind, card.step.state),
            (StepKind::Fix, StepState::Pending)
        );

        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.current, Some(StepKind::CrossReview), "{:?}", d.error);
        assert_eq!(d.findings[0].state, opw_domain::finding::FindingState::Fixing);

        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.current, Some(StepKind::FinalReview), "{:?}", d.error);
        assert_eq!(d.round, 1);
        assert_eq!(d.findings[0].state, opw_domain::finding::FindingState::Fixed);
        let system = flow.system(&ctx).unwrap();
        assert_eq!(system.agents[0].status, SessionStatus::Idle);
        assert_eq!(system.steps[5].round, Some(1));

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

        let d = run(&flow, &ctx, FlowAction::Finish);
        assert_eq!(d.status, FlowStatus::Done);
        assert!(flow.is_finished(1));
    }

    #[test]
    fn auto_advance_runs_until_the_next_gate_and_mediums_wait_for_the_user() {
        let Some(s) = setup() else { return };
        let (flow, _) = flow_with(&s, false, happy(MEDIUM));
        let mut config = Config::default();
        config.steps.auto_advance = true;
        let ctx = s.ctx(1, &config);
        flow.started(&ctx);
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.gate.map(|g| g.kind), Some(GateKind::PlanReview));
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::MediumTriage),
            "{:?}",
            d.error
        );
        assert!(d.gate.unwrap().message.starts_with("1 lỗi medium"));
        let medium = d
            .findings
            .iter()
            .find(|f| f.severity == Severity::Medium)
            .unwrap()
            .fingerprint
            .clone();

        assert_eq!(
            flow.act(ctx.clone(), FlowAction::FinishTriage).unwrap_err().code,
            "triage.open"
        );
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
        let d = run(&flow, &ctx, FlowAction::FinishTriage);
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);
        let accepted = d.findings.iter().find(|f| f.fingerprint == medium).unwrap();
        assert_eq!(accepted.reason.as_deref(), Some("chỉ là test"));
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
        flow.started(&ctx);
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
        flow.started(&ctx);
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
        flow.started(&one);
        flow.started(&two);
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
        flow.act(one.clone(), FlowAction::Stop).unwrap();
        let d = settle(&flow, &one);
        assert_eq!(d.error.unwrap().code, "step.stopped");
        assert_eq!(d.current, Some(StepKind::Analyze));
    }

    #[test]
    fn a_step_running_when_the_app_closed_can_be_retried() {
        let Some(s) = setup() else { return };
        let ctx = s.ctx(1, &Config::default());
        let mut saved = new_saved(&ctx);
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
        flow.started(&ctx);
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
        assert_eq!(d.current, Some(StepKind::Fix), "{:?}", d.error);
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
    fn push_blocks_fall_back_to_the_whole_text() {
        let v = push_view("just text", None);
        assert_eq!(v.blocks.len(), 1);
        assert_eq!(v.blocks[0].label, "Nội dung");
        assert!(push_view("", None).blocks.is_empty());
    }
}
