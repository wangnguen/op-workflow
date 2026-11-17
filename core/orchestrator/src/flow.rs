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

use crate::browsertest;
use crate::checks::{self, Check, CheckKind, CheckResult, CheckRunner, ProcessRunner};
use crate::devserver::{self, AppProcess, AppStarter, ShellStarter, Up};
use crate::harness;
use crate::quotagate;
use opw_agents::ModelChoice;
use opw_agents::claude::{self, ClaudeEvent, Denial, RunResult};
use opw_agents::codex::{self, CodexEvent};
use opw_agents::env::Account;
use opw_agents::procs::Sweep;
use opw_agents::run::{RunOutcome, RunSpec};
use opw_agents::text::{self, RawFinding};
use opw_agents::transcript;
use opw_config::{Config, RepoConfig, StepConfig, StepsConfig};
use opw_context::prompts::{self, FixItem};
use opw_domain::finding::{Severity, fingerprint};
use opw_domain::view::{
    AgentKind, AgentQuestionView, AgentView, AlertKind, ApiError, CarryOn, ChatItem, ChatKind, ChatView,
    Decision, DevServer, FindingDecision, FindingView, FlowAction, FlowError, FlowStatus, GateKind, GateView,
    LogLine, PlanQuestion, Provider, PushBlock, PushView, QuotaSnapshot, ReviewNote, RunState, RunSummary,
    RunView, SessionStatus, StepProposal, StepRef, StepView, TaskColumn, TaskDetail, TestCase, TestView,
    ToolEvent,
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
/// Optional steps the user turns on or off per task (the analysis proposes them).
const SWITCHABLE: [StepKind; 2] = [StepKind::Harness, StepKind::BrowserTest];
/// Times Claude may fix what Kiểm tra reports before the step stops for the user.
const CHECK_FIX_ROUNDS: usize = 2;
/// What a question left to the AI says in plan.md.
const AI_DECIDES: &str = "AI tự quyết";

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
            sweep: true,
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
    /// Code session: the analysis's, then Viết code / Sửa lỗi go on in it when the same AI
    /// runs them (`session_agent`; `None` = Claude, as every save before 6b).
    session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_agent: Option<AgentKind>,
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
    /// The latest analysis's questions with the user's answers.
    questions: Vec<PlanQuestion>,
    /// The latest analysis's advice on Harness / Test trình duyệt.
    proposals: Vec<StepProposal>,
    /// Optional steps the user turned on or off by hand: a new analysis no longer changes them.
    picked: Vec<StepKind>,
    /// "Tôi code xong" asked for the review: run it after Kiểm tra even without "Tự chạy".
    review_after_checks: bool,
    /// Harness (step 5): what the AI built and how it ran last.
    harness: Option<HarnessState>,
    /// Test trình duyệt (step 10): how it went last.
    browser: Option<BrowserState>,
    /// The usage limit ended a run in the middle: which, and when it can go on.
    cut: Option<CutRun>,
    /// Test trình duyệt: the dev server the user confirmed for this task (else the repo's).
    #[serde(skip_serializing_if = "Option::is_none")]
    dev_server: Option<DevServer>,
    /// The AI of a step stopped to ask the user (question gate), and the answer once given.
    #[serde(skip_serializing_if = "Option::is_none")]
    ask: Option<AskState>,
}

/// What the AI asked at the question gate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AskState {
    step: Option<StepKind>,
    question: String,
    url: String,
    /// The session to go on in once the user answered.
    session: String,
    /// What the user said (`Some("")` = they only did what was asked).
    answer: Option<String>,
}

/// The test's Chrome window and the dev server the app started for a task: kept open while its
/// AI waits for the user at the question gate, stopped when the test ends.
#[derive(Default)]
struct BrowserSession {
    chrome: Option<(Box<dyn AppProcess>, u16)>,
    server: Option<Box<dyn AppProcess>>,
}

impl BrowserSession {
    fn close(self, inner: &Inner, wp_id: u64) {
        let mut parts = Vec::new();
        if let Some((chrome, _)) = self.chrome {
            let _ = chrome.stop();
            parts.push("cửa sổ Chrome test".to_owned());
        }
        if let Some(server) = self.server {
            parts.push(match server.stop() {
                Some(what) => format!("dev server ({what})"),
                None => "dev server".to_owned(),
            });
        }
        if !parts.is_empty() {
            let line = format!("Đã tắt {}", parts.join(" và "));
            inner.log(wp_id, Some(StepKind::BrowserTest), "info", &line, false);
        }
    }
}

impl Saved {
    /// The code session when `agent` wrote it: another AI cannot go on in it.
    fn session_of(&self, agent: AgentKind) -> Option<String> {
        self.session
            .clone()
            .filter(|_| self.session_agent.unwrap_or(AgentKind::Claude) == agent)
    }

    fn set_session(&mut self, id: &str, agent: AgentKind) {
        if !id.is_empty() {
            self.session = Some(id.to_owned());
            self.session_agent = Some(agent);
        }
    }
}

/// A run the usage limit ended in the middle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CutRun {
    step: StepKind,
    provider: Provider,
    /// Claude's session of the cut run: Viết code / Sửa lỗi go on in it.
    session: Option<String>,
    /// When the limit resets (`None` = unknown: the quota gauges decide).
    until: Option<String>,
    /// When the run was cut (or Tự chạy stopped). With `until` unknown, only a gauge reading
    /// taken after this, and at least [`UNKNOWN_RESET_WAIT_SECS`] later, ends the wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<String>,
    /// The reset was announced (or the step started again).
    #[serde(default)]
    notified: bool,
}

/// The shortest wait when the reset time of a limit is unknown: a stale gauge that still shows
/// room must not retry the step on every poll.
const UNKNOWN_RESET_WAIT_SECS: f64 = 5.0 * 60.0;

/// The task's harness: the command the app runs again after each fix, and its last result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct HarnessState {
    /// Program and arguments, run from `dir` (the task's scratch folder).
    run: Vec<String>,
    dir: String,
    passed: bool,
    /// What the AI said about it.
    summary: String,
    /// Failed cases first.
    cases: Vec<harness::Case>,
    at: String,
    runs: u32,
    /// The broken copy made it FAIL (`None` = not reported).
    negative: Option<bool>,
    negative_detail: String,
    /// Case name → repository file, for the findings.
    files: Vec<(String, String)>,
    /// Kept in the repo's library: that folder, and the library itself.
    library: Option<String>,
    library_root: Option<String>,
    /// The AI found it worth keeping, but it did not PASS yet: kept once it does.
    pending: Option<harness::LibraryPick>,
}

/// The task's browser test (step 10): its last result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct BrowserState {
    passed: bool,
    summary: String,
    /// Where the AI opened the app.
    url: String,
    /// NG cases first.
    cases: Vec<browsertest::Case>,
    at: String,
    runs: u32,
}

impl BrowserState {
    fn view(&self) -> TestView {
        TestView {
            step: StepKind::BrowserTest,
            passed: self.passed,
            summary: self.summary.clone(),
            cases: self
                .cases
                .iter()
                .map(|c| TestCase {
                    name: c.name.clone(),
                    ok: c.ok,
                    detail: c.detail.clone(),
                    screenshot: c.screenshot.clone(),
                })
                .collect(),
            at: self.at.clone(),
            runs: self.runs,
            command: None,
            dir: None,
            negative: None,
            negative_detail: String::new(),
            library: None,
            url: Some(self.url.clone()).filter(|u| !u.is_empty()),
        }
    }
}

impl HarnessState {
    fn command(&self) -> String {
        self.run.join(" ")
    }

    fn view(&self) -> TestView {
        TestView {
            step: StepKind::Harness,
            passed: self.passed,
            summary: self.summary.clone(),
            cases: self
                .cases
                .iter()
                .map(|c| TestCase {
                    name: c.name.clone(),
                    ok: c.ok,
                    detail: c.detail.clone(),
                    screenshot: None,
                })
                .collect(),
            at: self.at.clone(),
            runs: self.runs,
            command: Some(self.command()).filter(|c| !c.is_empty()),
            dir: Some(self.dir.clone()).filter(|d| !d.is_empty()),
            negative: self.negative,
            negative_detail: self.negative_detail.clone(),
            library: self.library.clone(),
            url: None,
        }
    }
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
    /// What the step's AI runs left running and the app killed (dev servers, Chrome).
    swept: Vec<String>,
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

/// The AI a step runs with: what the user picked in Settings (`steps.<step>.agent`), else
/// [`agent_default`].
pub fn agent_for(step: StepKind, config: &Config) -> AgentKind {
    match step {
        // the app runs the commands; the AI of Sửa lỗi only steps in to fix what they report
        StepKind::Checks => AgentKind::Shell,
        StepKind::Intake => AgentKind::Claude,
        // Codex only after a real run shows Chrome DevTools MCP works in `codex exec` (6d)
        StepKind::BrowserTest => AgentKind::Claude,
        _ => picked_agent(step, config).unwrap_or_else(|| agent_default(step, config)),
    }
}

/// The agent written in `steps.<step>.agent`, if it names one.
fn picked_agent(step: StepKind, config: &Config) -> Option<AgentKind> {
    match step_config(step, &config.steps)?.agent.as_deref().map(str::trim) {
        Some("claude") => Some(AgentKind::Claude),
        Some("codex") => Some(AgentKind::Codex),
        _ => None,
    }
}

/// The AI of a step nobody picked one for: Claude, except Review chéo, which takes the other AI
/// than Viết code's so the change gets a second pair of eyes.
pub fn agent_default(step: StepKind, config: &Config) -> AgentKind {
    match step {
        StepKind::CrossReview => match agent_for(StepKind::Implement, config) {
            AgentKind::Codex => AgentKind::Claude,
            _ => AgentKind::Codex,
        },
        StepKind::Checks => AgentKind::Shell,
        _ => AgentKind::Claude,
    }
}

/// Steps whose agent can be switched in Settings.
pub fn agent_switchable(step: StepKind) -> bool {
    !matches!(step, StepKind::Intake | StepKind::Checks | StepKind::BrowserTest)
}

/// The permission mode picked for a step (`steps.<step>.permissionMode`).
pub fn permission_mode(step: StepKind, config: &Config) -> Option<String> {
    step_config(step, &config.steps)?
        .permission_mode
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
}

/// What the app passes when no mode is picked: Claude edits without asking in the steps that
/// write (Bash still only from the allow list); Codex writes in its workspace there and reads
/// only elsewhere. `None` = no flag (Claude's own default, with the app's tool and allow lists).
pub fn permission_default(step: StepKind, agent: AgentKind) -> Option<&'static str> {
    let writes = matches!(
        step,
        StepKind::Implement | StepKind::Fix | StepKind::FixCross | StepKind::Harness
    );
    match agent {
        AgentKind::Claude if writes => Some("acceptEdits"),
        AgentKind::Codex if writes => Some("workspace-write"),
        AgentKind::Codex => Some("read-only"),
        _ => None,
    }
}

/// The dev server Test trình duyệt uses: the task's own, else the one remembered for the repo
/// (`true`). `None` = the AI starts the app itself.
fn dev_server_of(ctx: &TaskCtx, saved: &Saved) -> Option<(DevServer, bool)> {
    let usable = |d: &DevServer| !d.url.trim().is_empty() && devserver::check(d).is_ok();
    saved
        .dev_server
        .clone()
        .filter(usable)
        .map(|d| (d, false))
        .or_else(|| ctx.repo_config().dev_server.filter(usable).map(|d| (d, true)))
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
        // Kiểm tra runs no AI
        StepKind::Intake | StepKind::Checks => return None,
        StepKind::Harness => &steps.harness,
        StepKind::BrowserTest => &steps.browser_test,
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
        StepKind::ReviewLocal | StepKind::FinalReview if agent == AgentKind::Codex => {
            "prompt review của app, codex exec chỉ đọc".into()
        }
        StepKind::ReviewLocal | StepKind::FinalReview if has("review-local.md") => {
            "/review-local của repo".into()
        }
        StepKind::ReviewLocal | StepKind::FinalReview => "prompt review của app".into(),
        StepKind::CrossReview if agent == AgentKind::Codex => "codex exec, chỉ đọc".into(),
        StepKind::CrossReview => "prompt review của app".into(),
        StepKind::PushSummary if agent == AgentKind::Codex => "prompt push summary của app (Codex)".into(),
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
        // each command and each AI fix round of Kiểm tra gets this much
        StepKind::Checks => return (2.0, Duration::from_secs(15 * 60), "checks"),
        StepKind::Harness => (&steps.harness, 2.0, 30, "harness"),
        StepKind::BrowserTest => (&steps.browser_test, 3.0, 30, "browserTest"),
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
        resets_at: None,
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
        resets_at: None,
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
    // a harness or browser test settles only what it found itself; a review leaves theirs
    let test_step = |key: Option<&str>| matches!(key, Some("harness" | "browserTest"));
    let ours = |old: &FindingRow| {
        if test_step(Some(step_key(step))) {
            old.step.as_deref() == Some(step_key(step))
        } else {
            !test_step(old.step.as_deref())
        }
    };
    for old in existing.values() {
        if old.state == "fixing" && ours(old) && !seen.contains(&old.fingerprint) {
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
    // Kiểm tra lets Claude fix what the commands report
    !matches!(
        step,
        StepKind::Implement | StepKind::Checks | StepKind::Fix | StepKind::FixCross
    )
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
/// The latest quota reading of a provider (the live gauges).
type QuotaSource = Arc<dyn Fn(Provider) -> Option<QuotaSnapshot> + Send + Sync>;

struct Inner {
    tasks: Mutex<HashMap<u64, Entry>>,
    store: Arc<Mutex<Store>>,
    launcher: Arc<dyn Launcher>,
    /// `<data dir>/runs`: raw output of each run, for debugging.
    runs_dir: Option<PathBuf>,
    quota_sink: Mutex<Option<QuotaSink>>,
    alert_sink: Mutex<Option<AlertSink>>,
    /// Runs the Kiểm tra commands (tests swap in a fake).
    checker: Mutex<Arc<dyn CheckRunner>>,
    /// Starts the dev server of Test trình duyệt (a fake in tests).
    starter: Mutex<Arc<dyn AppStarter>>,
    /// Chrome windows and dev servers kept open while an AI waits for the user.
    browsers: Mutex<HashMap<u64, BrowserSession>>,
    /// The user quit the app: running steps were stopped and no new one starts.
    quitting: AtomicBool,
    quota_source: Mutex<Option<QuotaSource>>,
}

/// The workflow engine shared by the Tauri commands.
#[derive(Clone)]
pub struct Flow {
    inner: Arc<Inner>,
}

/// A quota wait that ended: retry a cut step, or start the step paused before launch.
#[derive(Debug, PartialEq)]
pub struct QuotaWake {
    pub wp_id: u64,
    pub auto: Option<bool>,
    pub retry: bool,
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
    /// Permission mode picked in Settings; `None` = the app's own flags for the step.
    mode: Option<String>,
}

/// What an AI run gave back, whichever CLI ran it.
struct AiRun {
    text: String,
    /// Claude session / Codex thread (empty when the CLI kept none).
    session: String,
    /// USD; Codex reports none.
    cost: Option<f64>,
    /// Calls refused by the allow list (Claude only).
    denials: usize,
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
                // tasks from before Kiểm tra could run: turn it on where it is still ahead
                let checks_ahead = saved.workflow.state_of(StepKind::Checks) == StepState::Pending
                    && !saved.workflow.uses(StepKind::Checks);
                if checks_ahead {
                    saved.workflow.set_uses(StepKind::Checks, true);
                }
                let entry = Entry {
                    saved,
                    run: None,
                    log,
                };
                if checks_ahead {
                    if let Ok(json) = serde_json::to_string(&entry.saved) {
                        let _ = s.save_flow(wp_id, &json, &at);
                    }
                }
                tasks.insert(wp_id, entry);
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
                checker: Mutex::new(Arc::new(ProcessRunner)),
                starter: Mutex::new(Arc::new(ShellStarter)),
                browsers: Mutex::new(HashMap::new()),
                quitting: AtomicBool::new(false),
                quota_source: Mutex::new(None),
            }),
        }
    }

    /// What runs the Kiểm tra commands (the real processes by default).
    pub fn set_check_runner(&self, runner: Arc<dyn CheckRunner>) {
        if let Ok(mut c) = self.inner.checker.lock() {
            *c = runner;
        }
    }

    /// Replace what starts the dev server of Test trình duyệt (tests).
    pub fn set_app_starter(&self, starter: Arc<dyn AppStarter>) {
        if let Ok(mut s) = self.inner.starter.lock() {
            *s = starter;
        }
    }

    /// Where "a step ended and the task waits" goes (the bell and Windows notifications).
    pub fn set_alert_sink(&self, sink: impl Fn(FlowAlert) + Send + Sync + 'static) {
        if let Ok(mut s) = self.inner.alert_sink.lock() {
            *s = Some(Arc::new(sink));
        }
    }

    /// Where the flow reads the quota (auto-run stops at the highest `quota.warnAt` level; a
    /// usage limit hit mid-step gets its reset time).
    pub fn set_quota_source(
        &self,
        source: impl Fn(Provider) -> Option<QuotaSnapshot> + Send + Sync + 'static,
    ) {
        if let Ok(mut s) = self.inner.quota_source.lock() {
            *s = Some(Arc::new(source));
        }
    }

    /// Tasks stopped by the usage limit whose limit has reset now (each returned once): the
    /// facade runs them again (with "Tự chạy") or rings the bell. `None` = the task follows
    /// `steps.autoAdvance`.
    pub fn quota_waits_due(&self, now: &str, stop_level: f64) -> Vec<QuotaWake> {
        let mut due = Vec::new();
        let mut tasks = self.inner.lock_tasks();
        for (wp_id, entry) in tasks.iter_mut() {
            let retry = entry.saved.workflow.phase() == Phase::Gate(GateKind::Error)
                && entry.saved.error.as_ref().is_some_and(|e| e.code == "quota.hit");
            let waiting = entry.run.is_none()
                && !entry.saved.finished
                && (retry || matches!(entry.saved.workflow.phase(), Phase::Running(_)));
            let Some(cut) = entry.saved.cut.as_ref().filter(|c| waiting && !c.notified) else {
                continue;
            };
            if entry.saved.workflow.current_step() != Some(cut.step) {
                continue;
            }
            let active = self
                .inner
                .store
                .lock()
                .ok()
                .and_then(|s| s.task(*wp_id).ok().flatten())
                .is_some_and(|t| t.column == "active");
            if !active {
                continue;
            }
            let snap = self
                .inner
                .quota_of(cut.provider)
                .map(|s| quotagate::current(s, now));
            let room = snap
                .as_ref()
                .and_then(quotagate::worst)
                .map(|w| w.used_pct < stop_level);
            let reset = match cut.until.as_deref() {
                Some(until) => {
                    opw_quota::seconds_between(until, now).is_some_and(|s| s >= 0.0) && room != Some(false)
                }
                // Unknown reset: a few minutes at least, then only a gauge read after the cut
                // counts (a stale reading with room would retry on every poll).
                None => {
                    let since = |at: &str, to: &str| opw_quota::seconds_between(at, to);
                    let waited = cut
                        .at
                        .as_deref()
                        .is_none_or(|at| since(at, now).is_some_and(|s| s >= UNKNOWN_RESET_WAIT_SECS));
                    let fresh = cut.at.as_deref().is_none_or(|at| {
                        snap.as_ref()
                            .is_some_and(|s| since(at, &s.observed_at).is_some_and(|d| d > 0.0))
                    });
                    waited && fresh && room == Some(true)
                }
            };
            if !reset {
                continue;
            }
            if let Some(c) = entry.saved.cut.as_mut() {
                c.notified = true;
            }
            push_log(entry, None, "info", "Hạn mức đã reset");
            self.inner.persist(*wp_id, entry);
            due.push(QuotaWake {
                wp_id: *wp_id,
                auto: entry.saved.auto,
                retry,
            });
        }
        due
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

    /// The app is quitting: stop every running step (each AI process with everything it
    /// started) and start no new one. Returns how many were running.
    pub fn stop_all(&self) -> usize {
        self.inner.quitting.store(true, Ordering::Relaxed);
        let open: Vec<(u64, BrowserSession)> = self
            .inner
            .browsers
            .lock()
            .map(|mut b| b.drain().collect())
            .unwrap_or_default();
        for (wp_id, session) in open {
            session.close(&self.inner, wp_id);
        }
        let tasks = self.inner.lock_tasks();
        let mut n = 0;
        for run in tasks.values().filter_map(|e| e.run.as_ref()) {
            run.stop.store(true, Ordering::Relaxed);
            n += 1;
        }
        n
    }

    /// Steps running right now, over all tasks.
    pub fn running_count(&self) -> usize {
        self.inner
            .lock_tasks()
            .values()
            .filter(|e| e.run.is_some())
            .count()
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
        // the task left the question gate another way: nothing waits on its Chrome window now
        let left = match &action {
            FlowAction::Finish { .. } | FlowAction::Reopen { .. } => true,
            FlowAction::SetUses { step, on } => *step == StepKind::BrowserTest && !on,
            _ => false,
        };
        if left {
            self.inner.close_browser(wp_id);
        }
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
                .then(|| {
                    entry
                        .saved
                        .error
                        .as_ref()
                        .filter(|e| e.code == "agent.signedOut")
                        .and_then(|e| e.provider)
                })
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
        // Keep the run rows and phase in the same snapshot: finish writes its note before
        // releasing this lock and making the step appear idle.
        let tasks = self.inner.lock_tasks();
        let runs = self.inner.runs(ctx.row.wp_id);
        let findings = self.inner.findings(ctx.row.wp_id);
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
            questions: entry.saved.questions.clone(),
            proposals: entry.saved.proposals.clone(),
            dev_server: dev_server_of(ctx, &entry.saved).map(|(d, _)| d),
            dev_server_from_repo: dev_server_of(ctx, &entry.saved).is_some_and(|(_, repo)| repo),
            agent_question: entry
                .saved
                .ask
                .as_ref()
                .filter(|_| entry.saved.workflow.phase() == Phase::Gate(GateKind::AgentQuestion))
                .map(|a| AgentQuestionView {
                    step: a.step.unwrap_or(StepKind::BrowserTest),
                    question: a.question.clone(),
                    url: Some(a.url.clone()).filter(|u| !u.is_empty()),
                    browser_open: self
                        .inner
                        .browsers
                        .lock()
                        .is_ok_and(|b| b.get(&ctx.row.wp_id).is_some_and(|s| s.chrome.is_some())),
                }),
            uses: SWITCHABLE
                .into_iter()
                .filter(|s| entry.saved.workflow.uses(*s))
                .collect(),
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
            tests: entry
                .saved
                .harness
                .iter()
                .map(HarnessState::view)
                .chain(entry.saved.browser.iter().map(BrowserState::view))
                .collect(),
            // the facade fills this from the live quota
            quota_note: None,
        }
    }

    /// The Chat tab: the transcript of one run (`None` = the latest). Reads the run's raw log;
    /// call it off the UI thread.
    pub fn chat(&self, ctx: &TaskCtx, run_id: Option<i64>) -> Result<ChatView, ApiError> {
        let runs = self.inner.runs(ctx.row.wp_id);
        let row = match run_id {
            Some(id) => runs.iter().find(|r| r.id == id),
            // the latest AI run (Kiểm tra's own command rows have no chat)
            None => runs
                .iter()
                .rev()
                .find(|r| r.agent.as_deref() != Some("shell"))
                .or(runs.last()),
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
    // Kiểm tra always runs (it passes as "bỏ qua" when the repo has nothing to check)
    workflow.set_uses(StepKind::Checks, true);
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
        _ if step == StepKind::PushSummary => None,
        AgentKind::Codex => Some(format!("codex resume {id}")),
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

/// "plan: 3 file sẽ sửa, 2 câu hỏi, đề xuất Harness" from the sections `prompts::analyze` asks
/// for, the questions and the steps the AI advises.
fn plan_note(plan: &str, questions: usize, proposals: &[StepProposal]) -> String {
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
    let mut parts = Vec::new();
    if files > 0 {
        parts.push(format!("{files} file sẽ sửa"));
    }
    if questions > 0 {
        parts.push(format!("{questions} câu hỏi"));
    }
    let advised: Vec<&str> = proposals
        .iter()
        .filter(|p| p.recommend)
        .map(|p| step_label(p.step))
        .collect();
    if !advised.is_empty() {
        parts.push(format!("đề xuất {}", advised.join(" + ")));
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
            let plan = ctx
                .read(PLAN_FILE)
                .filter(|p| !p.trim().is_empty())
                .ok_or_else(|| ApiError::new("plan.missing", "Chưa có plan.md để duyệt"))?;
            if phase != Phase::Gate(GateKind::PlanReview) {
                return Err(not_now("duyệt plan"));
            }
            let open = entry.saved.questions.iter().filter(|q| !q.answered()).count();
            if open > 0 {
                return Err(ApiError::new(
                    "plan.questions",
                    format!(
                        "Còn {open} câu hỏi chưa trả lời: trả lời hoặc chọn \"{AI_DECIDES}\" trước khi duyệt"
                    ),
                ));
            }
            if !entry.saved.questions.is_empty() {
                write_task_file(ctx, PLAN_FILE, &plan_with_answers(&plan, &entry.saved.questions))
                    .map_err(|e| ApiError::new(e.code, e.message))?;
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
        FlowAction::AnswerQuestion {
            id,
            answer,
            ai_decides,
        } => {
            if phase != Phase::Gate(GateKind::PlanReview) {
                return Err(not_now("trả lời câu hỏi của plan"));
            }
            let q = entry
                .saved
                .questions
                .iter_mut()
                .find(|q| q.id == *id)
                .ok_or_else(|| ApiError::new("plan.question", "Không thấy câu hỏi này nữa"))?;
            q.ai_decides = *ai_decides;
            q.answer = if *ai_decides {
                None
            } else {
                answer
                    .as_deref()
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned)
            };
            Ok(false)
        }
        FlowAction::DropHarness => {
            let kept = entry
                .saved
                .harness
                .as_ref()
                .and_then(|h| Some((h.library.clone()?, h.library_root.clone()?)));
            let Some((saved, root)) = kept else {
                return Err(ApiError::new(
                    "harness.library",
                    "Harness của task này không có trong thư viện",
                ));
            };
            harness::drop_saved(Path::new(&root), Path::new(&saved))
                .map_err(|e| ApiError::new("harness.library", format!("Không bỏ được khỏi thư viện: {e}")))?;
            if let Some(h) = entry.saved.harness.as_mut() {
                h.library = None;
            }
            push_log(
                entry,
                Some(StepKind::Harness),
                "info",
                &format!("Đã bỏ harness khỏi thư viện: {saved}"),
            );
            Ok(false)
        }
        FlowAction::SetDevServer { command, url, dir } => {
            if entry
                .run
                .as_ref()
                .is_some_and(|r| r.step == StepKind::BrowserTest)
            {
                return Err(ApiError::new(
                    "devserver.busy",
                    "Test trình duyệt đang chạy: bấm Dừng trước khi đổi lệnh chạy app",
                ));
            }
            let server = DevServer {
                command: command.trim().to_owned(),
                url: url.trim().to_owned(),
                dir: dir.trim().trim_matches(['/', '\\']).to_owned(),
            };
            devserver::check(&server).map_err(|e| ApiError::new("devserver.invalid", e))?;
            let text = if server.url.is_empty() {
                "Bỏ lệnh chạy app: Test trình duyệt để AI tự bật app".to_owned()
            } else if server.command.is_empty() {
                format!("Test trình duyệt dùng app đang chạy ở {}", server.url)
            } else {
                format!(
                    "Test trình duyệt: app chạy `{}`{} rồi mở {}",
                    server.command,
                    if server.dir.is_empty() {
                        String::new()
                    } else {
                        format!(" trong {}", server.dir)
                    },
                    server.url
                )
            };
            entry.saved.dev_server = (!server.url.is_empty()).then_some(server);
            push_log(entry, Some(StepKind::BrowserTest), "info", &text);
            Ok(false)
        }
        FlowAction::AnswerAgent { answer } => {
            if phase != Phase::Gate(GateKind::AgentQuestion) {
                return Err(not_now("trả lời AI"));
            }
            let answer = answer.trim().to_owned();
            let text = if answer.is_empty() {
                "Bạn đã làm việc AI hỏi: AI chạy tiếp".to_owned()
            } else {
                format!("Bạn trả lời AI: {}", short(&answer))
            };
            if let Some(ask) = entry.saved.ask.as_mut() {
                ask.answer = Some(answer);
            }
            let step = entry.saved.workflow.asking();
            event(entry, E::AgentAnswered)?;
            push_log(entry, step, "ok", &text);
            Ok(true)
        }
        FlowAction::SetUses { step, on } => {
            if !SWITCHABLE.contains(step) {
                return Err(ApiError::new(
                    "flow.uses",
                    format!("{} không bật/tắt theo task được", step_label(*step)),
                ));
            }
            let wf = &entry.saved.workflow;
            if phase == Phase::NotStarted || entry.saved.finished || wf.state_of(*step) != StepState::Pending
            {
                return Err(ApiError::new(
                    "flow.uses",
                    format!(
                        "Đã qua {}: chỉ bật/tắt được trước khi tới bước này",
                        step_label(*step)
                    ),
                ));
            }
            entry.saved.workflow.set_uses(*step, *on);
            if !entry.saved.picked.contains(step) {
                entry.saved.picked.push(*step);
            }
            let text = format!(
                "Bạn {} {} cho task này",
                if *on { "bật" } else { "tắt" },
                step_label(*step)
            );
            push_log(entry, Some(*step), "info", &text);
            Ok(false)
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
            entry.saved.review_after_checks = true;
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
                AfterFix::BrowserTest => {
                    "Bạn chọn chạy lại Test trình duyệt, sau đó Review chốt lại".to_owned()
                }
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

/// plan.md with the user's answers as its last section (an older answers section is replaced).
fn plan_with_answers(plan: &str, questions: &[PlanQuestion]) -> String {
    let body = match plan.find(prompts::ANSWERS_HEADING) {
        Some(at) => &plan[..at],
        None => plan,
    };
    let body = body.trim_end();
    if questions.is_empty() {
        return format!("{body}\n");
    }
    let mut out = format!("{body}\n\n{}\n", prompts::ANSWERS_HEADING);
    for q in questions {
        let answer = if q.ai_decides {
            AI_DECIDES
        } else {
            q.answer.as_deref().map_or("", str::trim)
        };
        out.push_str(&format!("\n- {}\n  → {answer}\n", q.text.trim()));
    }
    out
}

/// The analysis's questions; an earlier answer is kept when the same question comes back.
fn new_questions(raw: Vec<text::RawQuestion>, before: &[PlanQuestion]) -> Vec<PlanQuestion> {
    raw.into_iter()
        .enumerate()
        .map(|(i, q)| {
            let old = before.iter().find(|b| b.text.trim() == q.text.trim());
            PlanQuestion {
                id: format!("q{}", i + 1),
                text: q.text,
                options: q.options,
                answer: old.and_then(|o| o.answer.clone()),
                ai_decides: old.is_some_and(|o| o.ai_decides),
            }
        })
        .collect()
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
        Phase::Gate(GateKind::AgentQuestion) => (
            AlertKind::Gate,
            title_of(&format!("{} hỏi bạn", step_label(step))),
            entry
                .saved
                .ask
                .as_ref()
                .map_or_else(|| ctx.row.title.clone(), |a| a.question.clone()),
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
            let open = entry.saved.questions.iter().filter(|q| !q.answered()).count();
            if open > 0 {
                format!("Plan đã xong, AI hỏi {open} câu: trả lời (hoặc để AI tự quyết) rồi duyệt.")
            } else {
                "Plan đã xong: duyệt để AI viết code, góp ý để phân tích lại, hoặc tự code.".into()
            }
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
        GateKind::AgentQuestion => match &entry.saved.ask {
            Some(a) => format!("AI hỏi: {} Làm xong thì bấm Tiếp tục.", a.question),
            None => "AI đang hỏi bạn.".into(),
        },
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
        if self.quitting.load(Ordering::Relaxed) {
            return Err(ApiError::new(
                "app.quitting",
                "App đang thoát nên không chạy bước mới",
            ));
        }
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
        if let Some(cut) = entry.saved.cut.as_mut() {
            cut.notified = true;
            if cut.step != step {
                entry.saved.cut = None;
            }
        }
        let agent = ctx.agent_for(step);
        let choice = model_choice(step, &ctx.config);
        let mode = permission_mode(step, &ctx.config).filter(|_| agent != AgentKind::Shell);
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
            swept: Vec::new(),
        });
        entry.saved.in_flight = true;
        entry.saved.error = None;
        let how = [
            Some(agent_name(agent).to_owned()),
            Some(model_text(choice.model.as_deref(), choice.effort.as_deref())).filter(|m| !m.is_empty()),
            mode.clone().map(|m| format!("quyền {m}")),
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
                mode,
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
        let quitting = self.quitting.load(Ordering::Relaxed);
        let result = result.map_err(|err| match err.code.as_str() {
            "step.stopped" if quitting => flow_error(
                "step.stopped",
                "App đã thoát hẳn khi bước này đang chạy nên AI đã dừng. Bấm Thử lại để chạy lại.",
            ),
            _ => err,
        });
        // the run's note is on its row before the task stops showing "running"
        let (early, swept) = self
            .lock_tasks()
            .get(&wp_id)
            .and_then(|e| e.run.as_ref())
            .map(|r| (r.run_id, r.swept.join("; ")))
            .unwrap_or_default();
        let with_sweep = |note: String| match (note.is_empty(), swept.is_empty()) {
            (_, true) => note,
            (true, false) => swept.clone(),
            (false, false) => format!("{note}; {swept}"),
        };
        let early_note = with_sweep(match &result {
            Ok(done) => done.note.clone(),
            Err(err) => short(&err.message),
        });
        if let (Some(id), Ok(store)) = (early.filter(|_| !early_note.is_empty()), self.store.lock()) {
            let _ = store.set_step_run_note(id, &early_note);
        }
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
                    if entry.saved.cut.as_ref().is_some_and(|c| c.step == step) {
                        entry.saved.cut = None;
                    }
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
        let note = with_sweep(note);
        push_log(entry, Some(step), kind, &text);
        self.persist(wp_id, entry);
        // Kiểm tra costs no tokens: it follows Viết code even without "Tự chạy"
        let after_checks = step == StepKind::Checks && std::mem::take(&mut entry.saved.review_after_checks);
        let rerun_harness = entry.saved.workflow.fixing() == Some(StepKind::Harness)
            && entry.saved.harness.as_ref().is_some_and(|h| !h.run.is_empty());
        let go_on = kind == "ok"
            && match entry.saved.workflow.phase() {
                Phase::Running(StepKind::Checks) => true,
                // after a fix of what the harness found, the app runs it again: no tokens either
                Phase::Running(StepKind::Harness) if rerun_harness => true,
                Phase::Running(_) => after_checks || ctx.auto(&entry.saved),
                _ => false,
            };
        // "Tự chạy" stops before a step that costs tokens once a window passed the highest level
        let quota_stop = match entry.saved.workflow.phase() {
            Phase::Running(next)
                if go_on
                    && next != StepKind::Checks
                    && !(next == StepKind::Harness
                        && entry.saved.harness.as_ref().is_some_and(|h| !h.run.is_empty())) =>
            {
                let provider = provider_of(ctx.agent_for(next));
                let (warn, stop) = quotagate::levels(&ctx.config.quota.warn_at);
                self.quota_of(provider)
                    .as_ref()
                    .and_then(quotagate::worst)
                    .filter(|w| w.used_pct >= warn)
                    .map(|w| {
                        (
                            w.used_pct >= stop,
                            quotagate::describe(provider, w),
                            next,
                            provider,
                        )
                    })
            }
            _ => None,
        };
        let mut alert = None;
        let mut go_on = go_on;
        match quota_stop {
            Some((true, said, next, provider)) => {
                go_on = false;
                let (_, stop) = quotagate::levels(&ctx.config.quota.warn_at);
                entry.saved.cut = Some(CutRun {
                    step: next,
                    provider,
                    session: None,
                    until: self
                        .quota_of(provider)
                        .as_ref()
                        .and_then(|s| quotagate::stop_reset(s, stop)),
                    at: Some(now()),
                    notified: false,
                });
                let line = format!(
                    "Tự chạy dừng trước {}: {said}. Bấm Chạy khi bạn muốn chạy tiếp.",
                    step_label(next)
                );
                push_log(entry, Some(next), "info", &line);
                alert = Some(FlowAlert {
                    wp_id,
                    task_id: ctx.row.display_id.clone(),
                    kind: AlertKind::Gate,
                    title: format!("{} · Tự chạy dừng vì hạn mức", ctx.row.display_id),
                    body: line,
                    tab: "progress",
                });
            }
            Some((false, said, next, _)) => {
                push_log(
                    entry,
                    Some(next),
                    "info",
                    &format!("{said}: Tự chạy vẫn chạy tiếp"),
                );
            }
            None => {}
        }
        if alert.is_none() {
            alert = (!go_on).then(|| stop_alert(ctx, step, entry, &text)).flatten();
        }
        self.persist(wp_id, entry);
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

    fn quota_of(&self, provider: Provider) -> Option<QuotaSnapshot> {
        let source = self.quota_source.lock().ok().and_then(|s| s.clone())?;
        source(provider).map(|s| quotagate::current(s, &now()))
    }

    /// The usage limit ended a run: remember it (session, reset time) and say so.
    fn quota_hit(
        &self,
        ctx: &TaskCtx,
        step: StepKind,
        provider: Provider,
        session: Option<&str>,
        text: &str,
    ) -> FlowError {
        let snap = self.quota_of(provider);
        let (_, stop) = quotagate::levels(&ctx.config.quota.warn_at);
        let at = now();
        let until = quotagate::reset_time(text, snap.as_ref(), stop, &at);
        let auto = self
            .lock_tasks()
            .get(&ctx.row.wp_id)
            .is_some_and(|e| ctx.auto(&e.saved));
        self.with_saved(ctx.row.wp_id, |s| {
            let session = session
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    s.cut
                        .as_ref()
                        .filter(|c| c.step == step && c.provider == provider)
                        .and_then(|c| c.session.clone())
                });
            s.cut = Some(CutRun {
                step,
                provider,
                session,
                until: until.clone(),
                at: Some(at),
                notified: false,
            });
        });
        let then = match (auto, until.is_some()) {
            (true, true) => "Tới giờ reset app tự chạy tiếp (Tự chạy đang bật).",
            (false, true) => "Tới giờ reset app báo chuông; bấm Thử lại để chạy tiếp.",
            (true, false) => {
                "Chưa biết giờ reset: app chờ ít nhất 5 phút và tới khi hạn mức đọc lại còn chỗ thì tự chạy tiếp; bấm Thử lại nếu muốn chạy ngay."
            }
            (false, false) => {
                "Chưa biết giờ reset: app báo chuông khi hạn mức đọc lại còn chỗ (sớm nhất sau 5 phút); bấm Thử lại nếu muốn chạy ngay."
            }
        };
        let carry = if step == StepKind::Implement || step.is_fix() {
            " AI sẽ tiếp tục trong phiên cũ, từ chỗ code đang dở."
        } else {
            ""
        };
        FlowError {
            code: "quota.hit".into(),
            message: format!(
                "Hết hạn mức {} giữa bước {}. {then}{carry}",
                quotagate::provider_name(provider),
                step_label(step)
            ),
            provider: Some(provider),
            resets_at: until,
        }
    }

    /// The CLI could not open the cut session again (it was cut before it was saved, or the
    /// CLI dropped it): forget it, so Thử lại runs the step from the start instead of failing
    /// on the same session forever.
    fn cut_session_lost(&self, wp_id: u64, step: StepKind, provider: Provider) -> FlowError {
        self.with_saved(wp_id, |s| {
            if let Some(cut) = s
                .cut
                .as_mut()
                .filter(|c| c.step == step && c.provider == provider)
            {
                cut.session = None;
            }
        });
        self.log(
            wp_id,
            Some(step),
            "error",
            "Không mở lại được phiên bị cắt vì hết hạn mức; app đã bỏ phiên đó",
            true,
        );
        flow_error(
            "session.lost",
            format!(
                "{} không mở lại được phiên bị cắt lúc hết hạn mức (phiên chưa kịp lưu). App đã bỏ phiên đó; bấm Thử lại để chạy lại bước {} từ đầu, phần code đã ghi vẫn còn trong repo.",
                quotagate::provider_name(provider),
                step_label(step)
            ),
        )
    }

    /// A usage limit cut a run of this step that has not finished since (its session may be gone).
    fn was_cut(&self, wp_id: u64, step: StepKind) -> bool {
        self.lock_tasks()
            .get(&wp_id)
            .and_then(|e| e.saved.cut.as_ref())
            .is_some_and(|c| c.step == step)
    }

    /// Keep the cut session until the step succeeds, including after a failed launch/retry.
    fn cut_session(&self, wp_id: u64, step: StepKind, provider: Provider) -> Option<String> {
        self.with_saved(wp_id, |s| {
            let cut = s.cut.as_ref()?;
            (cut.step == step && cut.provider == provider)
                .then(|| cut.session.clone())
                .flatten()
        })
        .flatten()
    }

    /// Stop the test's Chrome window and dev server of a task, if any are open.
    fn close_browser(&self, wp_id: u64) {
        let session = self.browsers.lock().ok().and_then(|mut b| b.remove(&wp_id));
        if let Some(session) = session {
            session.close(self, wp_id);
        }
    }

    /// What a run left running and the app killed: a line in the log (kept), and the step's
    /// note in Tiến trình says it. A port still taken afterwards is flagged.
    fn on_sweep(&self, wp_id: u64, step: StepKind, sweep: &Sweep) {
        let Some(text) = sweep_note(sweep) else { return };
        if let Some(run) = self.lock_tasks().get_mut(&wp_id).and_then(|e| e.run.as_mut()) {
            run.swept.push(text.clone());
        }
        let kind = if sweep.still_busy.is_empty() {
            "info"
        } else {
            "error"
        };
        self.log(wp_id, Some(step), kind, &format!("Dọn tiến trình: {text}"), true);
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
        let mut job = job.clone();
        if !job
            .args
            .iter()
            .any(|a| a == "--resume" || a == "--no-session-persistence")
        {
            if let Some(id) = self.cut_session(ctx.row.wp_id, step, Provider::Claude) {
                job.args.extend(["--resume".into(), id]);
            }
        }
        // the run goes on in the session a usage limit cut (added here or by the step)
        let resumes_cut = self
            .cut_session(ctx.row.wp_id, step, Provider::Claude)
            .is_some_and(|id| job.args.windows(2).any(|w| w[0] == "--resume" && w[1] == id));
        let job = &job;
        let stop: &AtomicBool = &claim.stop;
        use std::io::Write;
        let wp_id = ctx.row.wp_id;
        let (budget, timeout, key) = limits(step, &ctx.config);
        let (mut raw, log) = self.open_log(ctx, step, job);
        let run_id = self.begin_run(ctx, step, "claude", log.as_deref());
        let mut result: Option<RunResult> = None;
        let mut session = String::new();
        let mut used_model = String::new();
        let quota_hit =
            |session: &str, why: &str| self.quota_hit(ctx, step, Provider::Claude, Some(session), why);
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
        if let Ok(o) = &launched {
            self.on_sweep(wp_id, step, &o.sweep);
        }
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
                } else if resumes_cut
                    && (quotagate::looks_session_lost(&why)
                        || quotagate::looks_session_lost(&outcome.stderr_tail))
                {
                    Err(self.cut_session_lost(wp_id, step, Provider::Claude))
                } else if quotagate::looks_quota_hit(&why) || quotagate::looks_quota_hit(&outcome.stderr_tail)
                {
                    let sid = if r.session_id.is_empty() {
                        session.as_str()
                    } else {
                        r.session_id.as_str()
                    };
                    Err(quota_hit(sid, &format!("{why}\n{}", outcome.stderr_tail)))
                } else {
                    Err(flow_error(
                        "step.failed",
                        format!("Claude báo lỗi ({}): {}", r.subtype, short(&why)),
                    ))
                }
            }
            Some(r) => Ok(r),
            None if looks_signed_out(&outcome.stderr_tail) => Err(signed_out(AgentKind::Claude)),
            None if resumes_cut && quotagate::looks_session_lost(&outcome.stderr_tail) => {
                Err(self.cut_session_lost(wp_id, step, Provider::Claude))
            }
            None if quotagate::looks_quota_hit(&outcome.stderr_tail) => {
                Err(quota_hit(&session, &outcome.stderr_tail))
            }
            None => Err(flow_error(
                "step.failed",
                format!("Claude dừng mà không trả kết quả: {}", exit_why(&outcome)),
            )),
        }
    }

    /// Run Codex for one step; its last message and its thread id come back.
    fn run_codex(
        &self,
        ctx: &TaskCtx,
        step: StepKind,
        job: &Job,
        claim: &Claim,
    ) -> Result<(String, String), FlowError> {
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
        if let Ok(o) = &launched {
            self.on_sweep(wp_id, step, &o.sweep);
        }
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
        let quota_text = format!(
            "{}\n{}",
            failed.as_deref().unwrap_or_default(),
            outcome.stderr_tail
        );
        let resumes_cut = self
            .cut_session(wp_id, step, Provider::Codex)
            .is_some_and(|id| job.args.windows(2).any(|w| w[0] == "resume" && w[1] == id));
        if (failed.is_some() || !outcome.success())
            && resumes_cut
            && quotagate::looks_session_lost(&quota_text)
        {
            return Err(self.cut_session_lost(wp_id, step, Provider::Codex));
        }
        if (failed.is_some() || !outcome.success()) && quotagate::looks_quota_hit(&quota_text) {
            return Err(self.quota_hit(ctx, step, Provider::Codex, Some(&thread), &quota_text));
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
        Ok((last, thread))
    }

    /// Run the step's AI on `job` (built for that AI); text, session and cost come back alike.
    fn run_ai(&self, ctx: &TaskCtx, step: StepKind, job: &Job, claim: &Claim) -> Result<AiRun, FlowError> {
        if claim.agent == AgentKind::Codex {
            let (text, thread) = self.run_codex(ctx, step, job, claim)?;
            return Ok(AiRun {
                text,
                session: thread,
                cost: None,
                denials: 0,
            });
        }
        let r = self.run_claude(ctx, step, job, claim)?;
        Ok(AiRun {
            text: r.text,
            session: r.session_id,
            cost: Some(r.cost_usd),
            denials: r.denials.len(),
        })
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

/// "đã tắt 2 tiến trình AI để lại: node.exe (cổng 5173)", plus the ports still taken.
fn sweep_note(sweep: &Sweep) -> Option<String> {
    let mut text = sweep.describe()?;
    if !sweep.still_busy.is_empty() {
        let ports: Vec<String> = sweep.still_busy.iter().map(u16::to_string).collect();
        text.push_str(&format!(
            "; cổng {} vẫn bị chiếm sau khi tắt (xem bằng `netstat -ano | findstr :{}`)",
            ports.join(", "),
            ports[0]
        ));
    }
    Some(text)
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
            resets_at: None,
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

/// What Kiểm tra did.
struct ChecksDone {
    /// Something was checked (`false` = no command for the changed files).
    ran: bool,
    note: String,
    cost: f64,
}

/// Run the checks on the changed files; Claude fixes what they report (resuming the code
/// session) up to [`CHECK_FIX_ROUNDS`] times. In Kiểm tra itself every round gets its own row in
/// Tiến trình and errors left at the end stop the step; after a Sửa lỗi they only go in its note.
fn run_checks(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    repo: &Path,
    base: &str,
    step: StepKind,
) -> Result<ChecksDone, FlowError> {
    let wp_id = ctx.row.wp_id;
    let own_step = step == StepKind::Checks;
    let config = ctx.repo_config();
    let (budget, timeout, _) = limits(StepKind::Checks, &ctx.config);
    let checker = inner
        .checker
        .lock()
        .map(|c| c.clone())
        .map_err(|_| flow_error("checks.runner", "Không chạy được lệnh kiểm tra"))?;
    let dir = ctx.task_dir.join("checks");
    let _ = std::fs::create_dir_all(&dir);
    let mut cost = 0.0;
    let mut fixed = 0_usize;
    let mut round = 0_usize;
    loop {
        let changed = checks::changed_files(repo, base);
        let list = checks::plan(repo, &config.checks, &changed);
        if list.is_empty() {
            return Ok(ChecksDone {
                ran: false,
                note: "không có lệnh kiểm tra cho các file đã đổi (thêm `repos.<repo>.checks` trong config.json nếu cần)"
                    .into(),
                cost,
            });
        }
        let run_id = own_step
            .then(|| inner.begin_run(ctx, step, "shell", None))
            .flatten();
        let mut results: Vec<CheckResult> = Vec::new();
        let mut log = String::new();
        for check in &list {
            inner.on_tool(wp_id, step, "Kiểm tra", &check.label);
            let r = match checker.run(repo, check, timeout, &claim.stop) {
                Ok((code, output)) => checks::read(repo, check, code, &output, &changed),
                Err(why) => CheckResult {
                    label: check.label.clone(),
                    broken: Some(why),
                    ..CheckResult::default()
                },
            };
            log.push_str(&format!(
                "$ {} {}\n{}\n",
                check.program,
                check.args.join(" "),
                r.output
            ));
            results.push(r);
            if claim.stop.load(Ordering::Relaxed) {
                break;
            }
        }
        let _ = std::fs::write(dir.join(format!("{}-{}.log", step_key(step), round + 1)), &log);
        let clean = results.iter().all(CheckResult::ok);
        let mut note = checks::summary(&results);
        if fixed > 0 {
            note.push_str(&format!(" sau {fixed} lần AI sửa"));
        }
        if let (Some(id), Ok(store)) = (run_id, inner.store.lock()) {
            let _ = store.end_step_run(
                id,
                &StepRunEnd {
                    state: if clean { "done" } else { "failed" },
                    session_id: None,
                    cost_usd: None,
                    tokens_in: None,
                    tokens_out: None,
                    ended_at: &now(),
                    model: None,
                    effort: None,
                    denials: None,
                },
            );
            let _ = store.set_step_run_note(id, &note);
        }
        if claim.stop.load(Ordering::Relaxed) {
            return Err(flow_error(
                "step.stopped",
                "Bạn đã dừng bước này. Bấm Thử lại để chạy lại.",
            ));
        }
        inner.log(
            wp_id,
            Some(step),
            if clean { "ok" } else { "info" },
            &format!("Kiểm tra: {note}"),
            false,
        );
        if clean {
            return Ok(ChecksDone {
                ran: true,
                note,
                cost,
            });
        }
        let broken: Vec<&CheckResult> = results.iter().filter(|r| r.broken.is_some()).collect();
        if !broken.is_empty() || round >= CHECK_FIX_ROUNDS {
            if !own_step {
                return Ok(ChecksDone {
                    ran: true,
                    note: format!("{note} (xem tab Tiến trình; chạy lại Kiểm tra sau khi sửa tay)"),
                    cost,
                });
            }
            let message = match broken.first() {
                Some(b) => format!(
                    "Không chạy được {}: {}. Sửa lệnh trong `repos.<repo>.checks` (config.json) hoặc cài công cụ còn thiếu, rồi bấm Thử lại.",
                    b.label,
                    short(b.broken.as_deref().unwrap_or_default())
                ),
                None => format!(
                    "Kiểm tra còn lỗi sau {fixed} lần AI sửa: {}. Sửa tay rồi bấm Thử lại.",
                    short(&checks::report(&results))
                ),
            };
            return Err(flow_error("checks.failed", message));
        }
        // the AI of Sửa lỗi fixes them, in its code session when it has one
        let fixer = Claim {
            step,
            agent: agent_for(StepKind::Fix, &ctx.config),
            stop: claim.stop.clone(),
            choice: model_choice(StepKind::Fix, &ctx.config),
            mode: permission_mode(StepKind::Fix, &ctx.config),
        };
        let allowed = config.allow_bash.clone();
        let session = inner.with_saved(wp_id, |s| s.session_of(fixer.agent)).flatten();
        let prompt = prompts::fix_checks(&checks::report(&results), shell_of(&fixer, &allowed));
        let job = code_job(
            &fixer,
            session.as_deref(),
            &[],
            budget,
            &allowed,
            repo,
            timeout,
            prompt,
        );
        let r = inner.run_ai(ctx, step, &job, &fixer)?;
        inner.with_saved(wp_id, |s| s.set_session(&r.session, fixer.agent));
        cost += r.cost.unwrap_or_default();
        fixed += 1;
        round += 1;
    }
}

/// What the prompt of a code step says about the shell: Claude's allow list, unless the picked
/// mode lets commands through; Codex has none (its sandbox decides).
fn shell_of<'a>(claim: &Claim, allowed: &'a [String]) -> prompts::Shell<'a> {
    match claim.agent {
        AgentKind::Codex => prompts::Shell::Open,
        _ if claude::mode_opens_shell(claim.mode.as_deref()) => prompts::Shell::Open,
        _ => prompts::Shell::Allowed(allowed),
    }
}

/// A run of a step that writes code (Viết code, Sửa lỗi, fixing Kiểm tra, the harness) by the
/// claim's AI, with its model, effort and permission mode. `resume` = a session of that AI;
/// `dirs` = folders outside the repo it may write to; `allowed` = Claude's Bash allow list.
#[allow(clippy::too_many_arguments)]
fn code_job(
    claim: &Claim,
    resume: Option<&str>,
    dirs: &[String],
    budget: f64,
    allowed: &[String],
    repo: &Path,
    timeout: Duration,
    prompt: String,
) -> Job {
    let mode = claim.mode.as_deref();
    let (agent, args) = if claim.agent == AgentKind::Codex {
        let args = codex::code_args(resume, dirs);
        (
            Provider::Codex,
            codex::with_mode(codex::with_model(args, &claim.choice), mode),
        )
    } else {
        let args = if claim.step == StepKind::Harness {
            claude::harness_args(budget, allowed, dirs)
        } else {
            claude::code_args(resume, budget, allowed)
        };
        (
            Provider::Claude,
            claude::with_mode(claude::with_model(args, &claim.choice), mode),
        )
    };
    Job {
        agent,
        args,
        cwd: repo.to_path_buf(),
        stdin: Some(prompt),
        timeout,
    }
}

/// The test's Chrome profile for a repo (`<data dir>/browser-profiles/<repo path as a name>`):
/// a login made there is kept for the repo's next tasks. Without a data dir (tests) it sits in
/// the task folder.
fn browser_profile(inner: &Inner, ctx: &TaskCtx, repo: &Path) -> PathBuf {
    let name: String = repo
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    match inner.runs_dir.as_deref().and_then(Path::parent) {
        Some(data) => data.join("browser-profiles").join(name),
        None => ctx.task_dir.join(browsertest::DIR).join("profile"),
    }
}

/// The task's scratch folder (`<data dir>/scratch/<task>`): outside the repo and outside
/// `%TEMP%`. Without a data dir (tests) it sits in the task folder.
fn scratch_dir(inner: &Inner, ctx: &TaskCtx) -> PathBuf {
    match inner.runs_dir.as_deref().and_then(Path::parent) {
        Some(data) => data.join("scratch").join(&ctx.row.display_id),
        None => ctx.task_dir.join("harness").join("scratch"),
    }
}

/// Run the harness command once (the app, no tokens); its output goes to `<task>/harness/`.
fn harness_command(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    repo: &Path,
    state: &HarnessState,
) -> Result<(harness::Outcome, Vec<RawFinding>), FlowError> {
    let (_, timeout, _) = limits(StepKind::Harness, &ctx.config);
    let checker = inner
        .checker
        .lock()
        .map(|c| c.clone())
        .map_err(|_| flow_error("harness.run", "Không chạy được harness"))?;
    let (program, args) = state
        .run
        .split_first()
        .ok_or_else(|| flow_error("harness.run", "Harness không có lệnh để chạy"))?;
    let check = Check {
        label: "harness".into(),
        program: program.clone(),
        args: args.to_vec(),
        kind: CheckKind::Harness,
        dir: state.dir.clone(),
    };
    inner.on_tool(ctx.row.wp_id, StepKind::Harness, "Harness", &state.command());
    let (code, output) = checker.run(repo, &check, timeout, &claim.stop).map_err(|why| {
        flow_error(
            "harness.run",
            format!("Không chạy được harness (`{}`): {}", state.command(), short(&why)),
        )
    })?;
    if claim.stop.load(Ordering::Relaxed) {
        return Err(flow_error(
            "step.stopped",
            "Bạn đã dừng bước này. Bấm Thử lại để chạy lại.",
        ));
    }
    let dir = ctx.task_dir.join("harness");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(
        dir.join(format!("run-{}.log", state.runs + 1)),
        format!("$ {}\n{output}", state.command()),
    );
    let outcome = harness::read(code, &output);
    let raw = harness::findings(&outcome, &state.files, &output);
    inner.merge(ctx, StepKind::Harness, 0, "harness", &raw)?;
    Ok((outcome, raw))
}

/// Keep a run's result on the task: "PASS 5/5" / "FAIL 2/5".
fn keep_harness(inner: &Inner, wp_id: u64, mut state: HarnessState, outcome: &harness::Outcome) -> String {
    state.passed = outcome.passed;
    state.cases = harness::shown(outcome);
    state.at = now();
    state.runs += 1;
    let tally = harness::tally(outcome);
    inner.with_saved(wp_id, |s| s.harness = Some(state));
    if outcome.silent {
        format!("{tally} (harness không in dòng PASS/FAIL nào: chỉ theo mã thoát)")
    } else {
        tally
    }
}

/// After a Sửa lỗi: run the task's harness again when it has one. `None` = nothing to run.
fn harness_again(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    repo: &Path,
) -> Result<Option<String>, FlowError> {
    let wp_id = ctx.row.wp_id;
    let state = inner
        .with_saved(wp_id, |s| {
            s.harness.clone().filter(|_| s.workflow.uses(StepKind::Harness))
        })
        .flatten()
        .filter(|h| !h.run.is_empty() && Path::new(&h.dir).is_dir());
    let Some(state) = state else { return Ok(None) };
    let (outcome, _) = harness_command(inner, ctx, claim, repo, &state)?;
    let mut note = keep_harness(inner, wp_id, state, &outcome);
    if !outcome.passed {
        note.push_str(" (lỗi mới ở tab Lỗi review: Chọn thêm lỗi để sửa)");
    }
    inner.log(
        wp_id,
        Some(StepKind::Harness),
        "info",
        &format!("Harness lại: {note}"),
        false,
    );
    Ok(Some(note))
}

/// Step 5. The first time the AI builds the harness in the scratch folder (reusing the repo's
/// library), then the app runs it itself; after a fix of what it found only the app runs it
/// again. A FAIL stops at the findings gate like a review; a harness worth keeping goes to the
/// library.
fn run_harness(inner: &Inner, ctx: &TaskCtx, claim: &Claim, repo: &Path) -> Result<Done, FlowError> {
    let wp_id = ctx.row.wp_id;
    let (budget, timeout, _) = limits(StepKind::Harness, &ctx.config);
    let (saved, after_fix, checks) = inner
        .with_saved(wp_id, |s| {
            let checks: Vec<String> = s
                .proposals
                .iter()
                .filter(|p| p.step == StepKind::Harness)
                // the analysis's list comes as "- a\n- b"
                .flat_map(|p| {
                    p.checks
                        .lines()
                        .map(|l| l.trim().trim_start_matches("- ").to_owned())
                        .collect::<Vec<_>>()
                })
                .collect();
            (
                s.harness.clone(),
                s.workflow.fixing() == Some(StepKind::Harness),
                checks,
            )
        })
        .unwrap_or_default();
    let finish = |state: HarnessState,
                  (outcome, raw): (harness::Outcome, Vec<RawFinding>),
                  cost: Option<f64>,
                  extra: String| {
        let negative = match state.negative {
            Some(false) => "; negative control không FAIL: harness có thể không bắt được lỗi".to_owned(),
            _ => String::new(),
        };
        let mut state = state;
        let mut extra = extra;
        // a harness worth keeping goes to the library once it PASSes
        if let (true, Some(pick), Some(root)) =
            (outcome.passed, state.pending.clone(), state.library_root.clone())
        {
            let date = now().chars().take(10).collect::<String>();
            match harness::save(
                Path::new(&root),
                Path::new(&state.dir),
                &pick,
                &ctx.row.display_id,
                &date,
            ) {
                Ok(dest) => {
                    extra.push_str(&format!("; lưu vào thư viện: {}", dest.display()));
                    state.library = Some(dest.to_string_lossy().into_owned());
                }
                Err(e) => extra.push_str(&format!("; không lưu được vào thư viện: {e}")),
            }
            state.pending = None;
        }
        let tally = keep_harness(inner, wp_id, state, &outcome);
        // FAIL cases the user has not accepted or deferred stop at the findings gate
        let (event, _) = review_event(&inner.findings(wp_id), &raw);
        Done {
            event,
            note: format!("{tally}{negative}{extra}"),
            cost,
        }
    };
    // after a fix of what it found: the app runs the same harness again (no tokens)
    let again = saved
        .clone()
        .filter(|h| after_fix && !h.run.is_empty() && Path::new(&h.dir).is_dir());
    if let Some(state) = again {
        let ran = harness_command(inner, ctx, claim, repo, &state)?;
        return Ok(finish(
            state,
            ran,
            None,
            " (app chạy lại, không tốn token)".into(),
        ));
    }
    let repo_config = ctx.repo_config();
    let library = harness::library_dir(repo_config.harness_dir.as_deref(), repo);
    let repo_name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    harness::ensure_library(&library, &repo_name).map_err(|e| {
        flow_error(
            "harness.library",
            format!(
                "Không tạo được thư viện harness {}: {e}. Đổi thư mục ở Cài đặt → Repo rồi bấm Thử lại.",
                library.display()
            ),
        )
    })?;
    let scratch = scratch_dir(inner, ctx);
    std::fs::create_dir_all(&scratch).map_err(|e| flow_error("harness.scratch", e.to_string()))?;
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut allowed: Vec<String> = claude::HARNESS_BASH.iter().map(|s| (*s).to_owned()).collect();
    allowed.extend(repo_config.allow_bash.iter().cloned());
    let plan = ctx.read(PLAN_FILE).unwrap_or_default();
    let prompt = prompts::harness(
        &plan,
        &checks,
        &path(&scratch),
        &path(&library),
        shell_of(claim, &allowed),
    );
    let job = code_job(
        claim,
        None,
        &[path(&scratch), path(&library)],
        budget,
        &repo_config.allow_bash,
        repo,
        timeout,
        prompt,
    );
    let r = inner.run_ai(ctx, StepKind::Harness, &job, claim)?;
    let answer = harness::answer(&r.text).ok_or_else(|| {
        flow_error(
            "harness.parse",
            "AI không trả về kết quả harness (khối ```json có `result` và `run`). Xem tab Chat rồi bấm Thử lại.",
        )
    })?;
    let state = HarnessState {
        run: answer.run.clone(),
        dir: path(&scratch),
        summary: answer.summary.clone(),
        negative: answer.negative.as_ref().map(|(ok, _)| *ok),
        negative_detail: answer.negative.clone().map(|(_, d)| d).unwrap_or_default(),
        files: answer.files.clone(),
        library_root: Some(path(&library)),
        pending: answer.library.clone(),
        runs: saved.map_or(0, |s| s.runs),
        ..HarnessState::default()
    };
    // the app runs it itself: its result counts, and the command is known to work for later
    let ran = harness_command(inner, ctx, claim, repo, &state)?;
    Ok(finish(state, ran, r.cost, String::new()))
}

/// Step 11. Claude tries the change in Chrome through the Chrome DevTools MCP server only,
/// following the analysis's scenario; screenshots go to `<task>/browser-test/`. NG cases stop
/// at the findings gate; they are fixed in step 9 and the test runs again.
fn run_browser_test(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    repo: &Path,
    brief: &str,
) -> Result<Done, FlowError> {
    let wp_id = ctx.row.wp_id;
    let (budget, timeout, _) = limits(StepKind::BrowserTest, &ctx.config);
    let (scenario, runs) = inner
        .with_saved(wp_id, |s| {
            let scenario: Vec<String> = s
                .proposals
                .iter()
                .filter(|p| p.step == StepKind::BrowserTest)
                .flat_map(|p| {
                    p.checks
                        .lines()
                        .map(|l| l.trim().trim_start_matches("- ").to_owned())
                        .collect::<Vec<_>>()
                })
                .collect();
            (scenario, s.browser.as_ref().map_or(0, |b| b.runs))
        })
        .unwrap_or_default();
    let shots = ctx.task_dir.join(browsertest::DIR);
    let mcp = shots.join("mcp.json");
    std::fs::create_dir_all(&shots)
        .map_err(|e| flow_error("task.write", format!("Không tạo được {}: {e}", shots.display())))?;
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let repo_config = ctx.repo_config();
    let mut allowed: Vec<String> = claude::BROWSER_BASH.iter().map(|s| (*s).to_owned()).collect();
    allowed.extend(repo_config.allow_bash.iter().cloned());
    let plan = ctx.read(PLAN_FILE).unwrap_or_default();
    // the dev server the user confirmed: the app starts it (or finds it running) before the AI
    let server = inner
        .with_saved(wp_id, |s| dev_server_of(ctx, s))
        .flatten()
        .map(|(d, _)| d);
    // the user answered what the AI asked: go on in that session, with the window left open
    let answered = inner
        .with_saved(wp_id, |s| {
            s.ask
                .take()
                .filter(|a| a.answer.is_some() && !a.session.is_empty())
        })
        .flatten();
    let mut session = inner
        .browsers
        .lock()
        .ok()
        .and_then(|mut b| b.remove(&wp_id))
        .unwrap_or_default();
    let mut restarted = false;
    if let Some((chrome, _)) = session.chrome.as_mut() {
        if chrome.exited().is_some() {
            session.chrome = None;
            restarted = true;
        }
    }
    if session.server.as_mut().is_some_and(|p| p.exited().is_some()) {
        session.server = None;
    }
    if let (Some(d), None) = (&server, &session.server) {
        match start_app(inner, ctx, claim, repo, d, &shots) {
            Ok(p) => session.server = p,
            Err(e) => {
                session.close(inner, wp_id);
                return Err(e);
            }
        }
    }
    // the test's own Chrome window (profile per repo, so a login is kept for the next task)
    if session.chrome.is_none() {
        let starter = inner.starter.lock().map(|s| s.clone()).ok();
        let profile = browser_profile(inner, ctx, repo);
        let url = server.as_ref().map(|d| d.url.as_str()).unwrap_or_default();
        match starter.map(|s| s.open_browser(&profile, url)) {
            Some(Ok(opened)) => {
                inner.log(
                    wp_id,
                    Some(StepKind::BrowserTest),
                    "info",
                    &format!("Mở cửa sổ Chrome test (profile {})", profile.display()),
                    false,
                );
                session.chrome = Some(opened);
            }
            Some(Err(why)) => inner.log(
                wp_id,
                Some(StepKind::BrowserTest),
                "info",
                &format!("Không mở được cửa sổ Chrome test ({why}): AI dùng Chrome headless riêng"),
                false,
            ),
            None => {}
        }
    }
    let mcp_json = match &session.chrome {
        Some((_, port)) => claude::browser_mcp(*port),
        None => claude::BROWSER_MCP.to_owned(),
    };
    if let Err(e) = std::fs::write(&mcp, mcp_json) {
        session.close(inner, wp_id);
        return Err(flow_error(
            "task.write",
            format!("Không ghi được {}: {e}", mcp.display()),
        ));
    }
    let memory = browsertest::claude_projects().and_then(|p| browsertest::repo_memory(&p, repo));
    let app = match &server {
        Some(d) => prompts::BrowserApp::Running(&d.url),
        None => prompts::BrowserApp::Start(&allowed),
    };
    let prompt = match &answered {
        Some(a) => prompts::browser_answer(a.answer.as_deref().unwrap_or_default(), restarted),
        None => prompts::browser_test(
            &plan,
            brief,
            &scenario,
            &path(&shots),
            app,
            memory.as_deref().map(path).as_deref(),
        ),
    };
    let dirs: Vec<String> = memory.iter().map(|m| path(m)).collect();
    let mut args = claude::with_mode(
        claude::with_model(
            claude::browser_test_args(
                budget,
                &repo_config.allow_bash,
                &path(&mcp),
                server.is_none(),
                &dirs,
            ),
            &claim.choice,
        ),
        claim.mode.as_deref(),
    );
    if let Some(a) = &answered {
        args.splice(1..1, ["--resume".to_owned(), a.session.clone()]);
    }
    let job = Job {
        agent: Provider::Claude,
        args,
        cwd: repo.to_path_buf(),
        stdin: Some(prompt),
        timeout,
    };
    let ran = inner.run_claude(ctx, StepKind::BrowserTest, &job, claim);
    // the AI asks the user: keep the window (and the app) open until they answer
    if let Ok(r) = &ran {
        if let Some((question, url)) = browsertest::question(&r.text) {
            let open = session.chrome.is_some();
            if let Ok(mut b) = inner.browsers.lock() {
                b.insert(wp_id, session);
            }
            inner.with_saved(wp_id, |s| {
                s.ask = Some(AskState {
                    step: Some(StepKind::BrowserTest),
                    question: question.clone(),
                    url,
                    session: r.session_id.clone(),
                    answer: None,
                });
            });
            let where_ = if open {
                " (cửa sổ Chrome test vẫn mở)"
            } else {
                ""
            };
            return Ok(Done {
                event: WorkflowEvent::AgentAsked(StepKind::BrowserTest),
                note: format!("AI hỏi: {}{where_}", short(&question)),
                cost: Some(r.cost_usd),
            });
        }
    }
    // the test ended (or failed): close what the app opened for it
    let started_server = session.server.is_some();
    session.close(inner, wp_id);
    let app_note = match (started_server, &server) {
        (true, _) => "; app bật rồi tắt dev server",
        (false, Some(_)) => "; dùng app đang chạy sẵn",
        (false, None) => "",
    };
    let r = ran?;
    let answer = browsertest::answer(&r.text).ok_or_else(|| {
        flow_error(
            "browser.parse",
            "AI không trả về kết quả test (khối ```json có `cases`). Xem tab Chat rồi bấm Thử lại.",
        )
    })?;
    if answer.cases.is_empty() {
        return Err(flow_error(
            "browser.empty",
            "AI không báo ca test nào. Xem tab Chat rồi bấm Thử lại.",
        ));
    }
    let raw = browsertest::findings(&answer);
    let findings = inner.merge(ctx, StepKind::BrowserTest, 0, "browserTest", &raw)?;
    let (event, _) = review_event(&findings, &raw);
    let pictures = answer.cases.iter().filter(|c| c.screenshot.is_some()).count();
    let note = if pictures > 0 {
        format!("{}, {pictures} ảnh", browsertest::tally(&answer))
    } else {
        browsertest::tally(&answer)
    };
    let state = BrowserState {
        passed: answer.passed(),
        summary: answer.summary.clone(),
        url: answer.url.clone(),
        cases: browsertest::shown(&answer),
        at: now(),
        runs: runs + 1,
    };
    inner.with_saved(wp_id, |s| s.browser = Some(state));
    Ok(Done {
        event,
        note: format!("{note}{app_note}"),
        cost: Some(r.cost_usd),
    })
}

/// Test trình duyệt with a dev server: start it unless it already answers, and wait for its URL.
/// `Some` = the app started it and stops it when the step ends.
fn start_app(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    repo: &Path,
    server: &DevServer,
    shots: &Path,
) -> Result<Option<Box<dyn devserver::AppProcess>>, FlowError> {
    let wp_id = ctx.row.wp_id;
    let starter = inner
        .starter
        .lock()
        .map(|s| s.clone())
        .map_err(|_| flow_error("devserver.start", "Không bật được dev server"))?;
    let what = if server.command.is_empty() {
        server.url.as_str()
    } else {
        server.command.as_str()
    };
    inner.on_tool(wp_id, StepKind::BrowserTest, "Dev server", what);
    let log = shots.join("dev-server.log");
    match devserver::bring_up(&*starter, repo, server, &log, devserver::READY_WAIT, &claim.stop) {
        Ok(Up::Running) => {
            let line = format!("Dùng app đang chạy ở {} (app không tắt nó)", server.url);
            inner.log(wp_id, Some(StepKind::BrowserTest), "info", &line, false);
            Ok(None)
        }
        Ok(Up::Started(p, after)) => {
            let line = format!(
                "Dev server: app chạy `{}`, {} mở sau {} giây",
                server.command,
                server.url,
                after.as_secs()
            );
            inner.log(wp_id, Some(StepKind::BrowserTest), "info", &line, false);
            Ok(Some(p))
        }
        Err((code, message)) => Err(flow_error(code, message)),
    }
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
    // the code session, when this step's AI wrote it (another AI cannot go on in it)
    let (session, feedback, fixes, issue, note) = inner
        .with_saved(wp_id, |s| {
            (
                s.session_of(claim.agent),
                s.feedback.clone(),
                s.fixes,
                s.issue,
                s.note.clone(),
            )
        })
        .unwrap_or_default();
    let set_session = |id: &str| {
        inner.with_saved(wp_id, |s| s.set_session(id, claim.agent));
    };
    let codex = claim.agent == AgentKind::Codex;
    // the user's model / effort / permission mode for this step (nothing when left to the app)
    let cl =
        |args: Vec<String>| claude::with_mode(claude::with_model(args, &claim.choice), claim.mode.as_deref());
    let cx =
        |args: Vec<String>| codex::with_mode(codex::with_model(args, &claim.choice), claim.mode.as_deref());
    // a run the usage limit cut goes on in its session
    let codex_cut = |args: Vec<String>| match inner.cut_session(wp_id, step, Provider::Codex) {
        Some(id) => codex::with_resume(args, &id),
        None => args,
    };
    match step {
        StepKind::Intake => Ok(Done {
            event: E::StepSucceeded(StepKind::Intake),
            note: String::new(),
            cost: None,
        }),
        StepKind::Analyze => {
            let previous_plan = ctx.read(PLAN_FILE);
            let previous = feedback.as_deref().zip(previous_plan.as_deref());
            // answers to the previous plan's questions carry over into the new analysis
            let before = inner
                .with_saved(wp_id, |s| s.questions.clone())
                .unwrap_or_default();
            let answers: Vec<(String, String)> = before
                .iter()
                .filter(|q| q.answered())
                .map(|q| {
                    let a = if q.ai_decides {
                        AI_DECIDES.to_owned()
                    } else {
                        q.answer.clone().unwrap_or_default()
                    };
                    (q.text.clone(), a)
                })
                .collect();
            let answers: Vec<(&str, &str)> = answers.iter().map(|(q, a)| (q.as_str(), a.as_str())).collect();
            let mut prompt = prompts::analyze(&brief, previous, &answers);
            // notes from earlier work on the repo: where it runs, how it is tested
            let memory = browsertest::claude_projects().and_then(|p| browsertest::repo_memory(&p, repo));
            let dirs: Vec<String> = memory.iter().map(|m| m.to_string_lossy().into_owned()).collect();
            if let Some(dir) = dirs.first() {
                prompt.push_str(&prompts::memory_note(dir));
            }
            let analyze = if codex {
                job(
                    Provider::Codex,
                    codex_cut(cx(codex::analyze_args())),
                    Some(prompt),
                )
            } else {
                job(
                    Provider::Claude,
                    cl(claude::analyze_args(budget, &dirs)),
                    Some(prompt),
                )
            };
            let r = inner.run_ai(ctx, step, &analyze, claim)?;
            let answer = text::plan_answer(&r.text);
            if answer.plan.is_empty() {
                return Err(flow_error(
                    "plan.empty",
                    format!(
                        "{} không trả về plan. Xem Log rồi bấm Thử lại.",
                        agent_name(claim.agent)
                    ),
                ));
            }
            write_task_file(ctx, PLAN_FILE, &answer.plan)?;
            set_session(&r.session);
            let questions = new_questions(answer.questions, &before);
            let proposals: Vec<StepProposal> = answer
                .proposals
                .into_iter()
                .filter_map(|p| {
                    let step = match p.step {
                        "harness" => StepKind::Harness,
                        "browserTest" => StepKind::BrowserTest,
                        _ => return None,
                    };
                    Some(StepProposal {
                        step,
                        recommend: p.recommend,
                        reason: p.reason,
                        checks: p.checks,
                        dev_server: p.dev_server.map(|d| DevServer {
                            command: d.command,
                            url: d.url,
                            dir: d.dir,
                        }),
                    })
                })
                .collect();
            let note = plan_note(&answer.plan, questions.len(), &proposals);
            inner.with_saved(wp_id, |s| {
                // the AI's advice turns a step on or off unless the user already chose
                for p in &proposals {
                    if !s.picked.contains(&p.step) && s.workflow.state_of(p.step) == StepState::Pending {
                        s.workflow.set_uses(p.step, p.recommend);
                    }
                }
                s.questions = questions;
                s.proposals = proposals;
            });
            Ok(Done {
                event: E::StepSucceeded(StepKind::Analyze),
                note,
                cost: r.cost,
            })
        }
        StepKind::Implement => {
            let plan = ctx
                .read(PLAN_FILE)
                .ok_or_else(|| flow_error("plan.missing", "Không đọc được plan.md"))?;
            let allowed = ctx.repo_config().allow_bash;
            let shell = shell_of(claim, &allowed);
            // the usage limit cut the last run: go on in its session, from where the code is
            let cut = inner.cut_session(wp_id, step, provider_of(claim.agent));
            let mut prompt = match &cut {
                Some(_) => prompts::carry_on_cut(step_label(step), shell),
                None => prompts::implement(
                    &plan,
                    session.is_none().then_some(brief.as_str()),
                    shell,
                    note.as_deref(),
                ),
            };
            if cut.is_none() && inner.was_cut(wp_id, step) {
                prompt.push_str(prompts::CUT_BEFORE);
            }
            let resume = cut.as_deref().or(session.as_deref());
            let code = code_job(claim, resume, &[], budget, &allowed, repo, timeout, prompt);
            let r = inner.run_ai(ctx, step, &code, claim)?;
            set_session(&r.session);
            if note.is_some() {
                inner.with_saved(wp_id, |s| s.note = None);
            }
            Ok(Done {
                event: E::StepSucceeded(StepKind::Implement),
                note: format!("{}{}", change_note(repo), denied_note(r.denials)),
                cost: r.cost,
            })
        }
        StepKind::ReviewLocal | StepKind::FinalReview if codex => {
            // Codex cannot run the repo's /review-local: the app's review prompt, read only
            let raw = codex_review(
                inner,
                ctx,
                claim,
                &format!("codex-{}", step_key(step)),
                &base,
                &brief,
            )?;
            let findings = inner.merge(ctx, step, 0, "codex", &raw)?;
            let (event, note) = review_event(&findings, &raw);
            Ok(Done {
                event,
                note,
                cost: None,
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
            let shell = shell_of(claim, &allowed);
            let cut = inner.cut_session(wp_id, step, provider_of(claim.agent));
            // no session of this AI: it did not write the code, so it gets the plan and the brief
            let plan = ctx.read(PLAN_FILE).unwrap_or_default();
            let context = session.is_none().then_some((plan.as_str(), brief.as_str()));
            let mut prompt = match &cut {
                Some(_) => prompts::carry_on_cut(step_label(step), shell),
                None => prompts::fix(&list, shell, context),
            };
            if cut.is_none() && inner.was_cut(wp_id, step) {
                prompt.push_str(prompts::CUT_BEFORE);
            }
            let resume = cut.as_deref().or(session.as_deref());
            let code = code_job(claim, resume, &[], budget, &allowed, repo, timeout, prompt);
            let r = inner.run_ai(ctx, step, &code, claim)?;
            set_session(&r.session);
            let fixes = inner
                .with_saved(wp_id, |s| {
                    s.fixes += 1;
                    s.fixes
                })
                .unwrap_or(fixes + 1);
            // Kiểm tra again on what the fix changed; it belongs to this step (no own row)
            let again = run_checks(inner, ctx, claim, repo, &base, step)?;
            let mut again_note = if again.ran {
                format!("; Kiểm tra lại: {}", again.note)
            } else {
                String::new()
            };
            // and the task's harness (no tokens), unless the harness step itself runs next
            let harness_next = inner
                .with_saved(wp_id, |s| s.workflow.fixing() == Some(StepKind::Harness))
                .unwrap_or_default();
            if !harness_next {
                if let Some(h) = harness_again(inner, ctx, claim, repo)? {
                    again_note.push_str(&format!("; Harness lại: {h}"));
                }
            }
            let cost = match r.cost {
                Some(c) => Some(c + again.cost),
                None => (again.cost > 0.0).then_some(again.cost),
            };
            Ok(Done {
                event: E::StepSucceeded(step),
                note: format!(
                    "lần sửa thứ {fixes}: xử lý {} lỗi; {}{}{again_note}",
                    list.len(),
                    change_note(repo),
                    denied_note(r.denials)
                ),
                cost,
            })
        }
        StepKind::Checks => {
            let c = run_checks(inner, ctx, claim, repo, &base, step)?;
            if !c.ran {
                return Ok(Done {
                    event: E::StepSkipped(StepKind::Checks),
                    note: format!("bỏ qua: {}", c.note),
                    cost: None,
                });
            }
            Ok(Done {
                event: E::StepSucceeded(StepKind::Checks),
                note: c.note,
                cost: (c.cost > 0.0).then_some(c.cost),
            })
        }
        StepKind::Harness => run_harness(inner, ctx, claim, repo),
        StepKind::BrowserTest => run_browser_test(inner, ctx, claim, repo, &brief),
        StepKind::CrossReview => {
            let round = inner.round_of(wp_id);
            let (raw, source, cost) = if codex {
                let raw = codex_review(inner, ctx, claim, &format!("codex-r{round}"), &base, &brief)?;
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
            let push = if codex {
                // Codex cannot run the repo's /push-summary: the app's prompt
                let stdin = Some(prompts::push_summary(&base, issue));
                job(Provider::Codex, codex_cut(cx(codex::push_args())), stdin)
            } else if has_command {
                job(
                    Provider::Claude,
                    cl(claude::push_summary_args(issue, budget)),
                    None,
                )
            } else {
                let stdin = Some(prompts::push_summary(&base, issue));
                job(
                    Provider::Claude,
                    cl(claude::push_summary_default_args(budget)),
                    stdin,
                )
            };
            let r = inner.run_ai(ctx, step, &push, claim)?;
            if r.text.trim().is_empty() {
                return Err(flow_error(
                    "push.empty",
                    format!(
                        "{} không trả về commit message. Xem Log rồi bấm Thử lại.",
                        agent_name(claim.agent)
                    ),
                ));
            }
            write_task_file(ctx, PUSH_FILE, &r.text)?;
            Ok(Done {
                event: E::StepSucceeded(StepKind::PushSummary),
                note: "đã viết commit message + PR description".into(),
                cost: r.cost,
            })
        }
    }
}

/// A review by Codex (Review chéo, or Tự review / Review chốt when Settings picks Codex): the
/// app's review prompt, read only, the answer forced into the findings schema and written to
/// `<task>/review/<name>.json`. A run the usage limit cut goes on in its thread.
fn codex_review(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    name: &str,
    base: &str,
    brief: &str,
) -> Result<Vec<RawFinding>, FlowError> {
    let step = claim.step;
    let (_, timeout, _) = limits(step, &ctx.config);
    let repo = ctx
        .repo()
        .ok_or_else(|| flow_error("repo.missing", "Không thấy thư mục repo của task"))?;
    let dir = ctx.task_dir.join(REVIEW_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| flow_error("task.write", e.to_string()))?;
    let schema = dir.join("findings.schema.json");
    std::fs::write(&schema, codex::FINDINGS_SCHEMA).map_err(|e| flow_error("task.write", e.to_string()))?;
    let out = dir.join(format!("{name}.json"));
    let _ = std::fs::remove_file(&out);
    let mut args = codex::with_mode(
        codex::with_model(
            codex::cross_review_args(&schema.to_string_lossy(), &out.to_string_lossy()),
            &claim.choice,
        ),
        claim.mode.as_deref(),
    );
    if let Some(id) = inner.cut_session(ctx.row.wp_id, step, Provider::Codex) {
        // `exec` options (including the read-only sandbox) stay on the parent command
        args = codex::with_resume(args, &id);
    }
    let job = Job {
        agent: Provider::Codex,
        args,
        cwd: repo,
        stdin: Some(prompts::cross_review(base, brief)),
        timeout,
    };
    let (answer, _) = inner.run_codex(ctx, step, &job, claim)?;
    let text = std::fs::read_to_string(&out).unwrap_or(answer);
    let raw = serde_json::from_str::<serde_json::Value>(text.trim())
        .ok()
        .and_then(|v| text::findings_from_json(&v))
        .or_else(|| text::findings_from_text(&text))
        .ok_or_else(unreadable_review)?;
    save_review_notes(ctx, step, &text);
    Ok(raw)
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
        } else if has("--mcp-config") {
            "browser"
        } else if has("--add-dir") {
            "harness"
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
    fn quitting_stops_every_step_and_says_what_was_left_running() {
        let Some(s) = setup() else { return };
        let (flow, _) = flow_with(&s, false, |_, stop, _| {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            RunOutcome {
                stopped: true,
                sweep: Sweep {
                    killed: vec![(41, "node.exe".into()), (42, "chrome.exe".into())],
                    ports: vec![5173],
                    still_busy: vec![5173],
                },
                ..RunOutcome::default()
            }
        });
        let one = s.ctx(1, &Config::default());
        flow.started(&one, None).unwrap();
        flow.act(one.clone(), FlowAction::Run).unwrap();
        assert_eq!(flow.running_count(), 1);
        assert_eq!(flow.stop_all(), 1);
        let d = settle(&flow, &one);
        assert_eq!(flow.running_count(), 0);
        let err = d.error.unwrap();
        assert_eq!(err.code, "step.stopped");
        assert!(err.message.contains("thoát hẳn"), "{}", err.message);
        let note = d.runs.last().and_then(|r| r.note.clone()).unwrap_or_default();
        assert!(
            note.contains("đã tắt 2 tiến trình AI để lại: node.exe, chrome.exe (cổng 5173)")
                && note.contains("cổng 5173 vẫn bị chiếm"),
            "{note}"
        );
        assert!(
            d.log
                .iter()
                .any(|l| l.kind == "error" && l.text.starts_with("Dọn tiến trình:")),
            "{:?}",
            d.log
        );
        // no new step starts while the app quits
        assert_eq!(
            flow.act(one.clone(), FlowAction::Retry).unwrap_err().code,
            "app.quitting"
        );
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

    /// Like [`happy`], but Codex may write code too: a Codex job without `-o` edits a.ts and
    /// answers on thread `t-<n>`.
    fn codex_codes() -> impl ScriptFn {
        let reviews = AtomicUsize::new(0);
        let threads = AtomicUsize::new(0);
        let claude = happy(CLEAN);
        move |job, stop, out| {
            if job.agent != Provider::Codex {
                // the first Claude review finds one high, the rest are clean
                if kind(job) == "review" {
                    let n = reviews.fetch_add(1, Ordering::SeqCst);
                    out(&result_line(if n == 0 { HIGH } else { CLEAN }));
                    return ok();
                }
                return claude(job, stop, out);
            }
            if let Some(i) = job.args.iter().position(|a| a == "-o") {
                std::fs::write(&job.args[i + 1], "{\"findings\":[]}").unwrap();
                return ok();
            }
            let n = threads.fetch_add(1, Ordering::SeqCst);
            out(&format!(r#"{{"type":"thread.started","thread_id":"t-{n}"}}"#));
            let plan = !job
                .args
                .iter()
                .any(|a| a == "workspace-write" || a == "danger-full-access");
            let text = if job.args.iter().any(|a| a == "--ephemeral") {
                PUSH.to_owned()
            } else if plan {
                "## Nguyên nhân\n`a.ts:1`\n## Đề xuất\nguard".to_owned()
            } else {
                let file = job.cwd.join("a.ts");
                let old = std::fs::read_to_string(&file).unwrap();
                std::fs::write(&file, format!("{old}// codex\n")).unwrap();
                "đã sửa a.ts".to_owned()
            };
            out(&serde_json::json!({"type": "item.completed", "item": {"type": "agent_message", "text": text}}).to_string());
            out(
                r#"{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":50}}"#,
            );
            ok()
        }
    }

    fn flag(job: &Job, name: &str) -> Option<String> {
        job.args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
    }

    #[test]
    fn codex_can_plan_and_write_the_code_and_claude_then_cross_reviews_it() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, codex_codes());
        let config = Config::from_value(&serde_json::json!({
            "steps": {
                "autoAdvance": true,
                "analyze": {"agent": "codex"},
                "implement": {"agent": "codex", "model": "gpt-x"},
                "fix": {"agent": "codex", "permissionMode": "danger-full-access"}
            }
        }));
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::PlanReview),
            "{:?}",
            d.error
        );
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
        let codex: Vec<&Job> = jobs.iter().filter(|j| j.agent == Provider::Codex).collect();
        // Phân tích, Viết code, Sửa lỗi: nothing else ran on Codex
        assert_eq!(codex.len(), 3, "{jobs:#?}");
        let (analyze, implement, fix) = (codex[0], codex[1], codex[2]);
        assert_eq!(flag(analyze, "--sandbox").as_deref(), Some("read-only"));
        // Viết code goes on in the analysis's thread, so it needs no brief
        assert_eq!(flag(implement, "--sandbox").as_deref(), Some("workspace-write"));
        assert_eq!(flag(implement, "-m").as_deref(), Some("gpt-x"));
        assert_eq!(
            &implement.args[implement.args.len() - 3..],
            ["resume", "t-0", "-"]
        );
        let prompt = implement.stdin.as_deref().unwrap();
        assert!(
            !prompt.contains("<brief>") && !prompt.contains("allow list"),
            "{prompt}"
        );
        assert!(prompt.contains("never run git commands that write"));
        // Sửa lỗi: the picked permission mode, in the code thread
        assert_eq!(flag(fix, "--sandbox").as_deref(), Some("danger-full-access"));
        assert_eq!(&fix.args[fix.args.len() - 3..], ["resume", "t-1", "-"]);
        // nobody picked the cross review's AI: Viết code was Codex, so Claude reviews it
        assert!(
            d.log
                .iter()
                .any(|l| l.text.starts_with("Bắt đầu Review chéo (Claude")),
            "{:?}",
            d.log
        );
        assert!(
            d.log.iter().any(|l| l
                .text
                .starts_with("Bắt đầu Sửa lỗi (Codex · quyền danger-full-access")),
            "{:?}",
            d.log
        );
        let implement_row = &d.steps[StepKind::Implement.index() as usize - 1];
        assert_eq!(implement_row.cost_usd, None);
    }

    #[test]
    fn codex_reviews_and_writes_the_push_summary_with_the_apps_prompts() {
        let Some(s) = setup() else { return };
        // the repo has both commands, which Codex cannot run
        let commands = s.repo.join(".claude").join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join("review-local.md"), "review").unwrap();
        std::fs::write(commands.join("push-summary.md"), "push").unwrap();
        let (flow, fake) = flow_with(&s, false, codex_codes());
        let config = Config::from_value(&serde_json::json!({
            "steps": {
                "autoAdvance": true,
                "reviewLocal": {"agent": "codex"},
                "crossReview": {"agent": "codex"},
                "finalReview": {"agent": "codex", "permissionMode": "workspace-write"},
                "pushSummary": {"agent": "codex"}
            }
        }));
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);
        let push = std::fs::read_to_string(ctx.file(PUSH_FILE)).unwrap();
        assert!(push.contains("fix: org (#279)"), "{push}");

        let jobs = fake.jobs.lock().unwrap().clone();
        let codex: Vec<&Job> = jobs.iter().filter(|j| j.agent == Provider::Codex).collect();
        // Tự review, Review chéo, Review chốt, Push summary
        assert_eq!(codex.len(), 4, "{jobs:#?}");
        for review in &codex[..3] {
            assert!(review.args.iter().any(|a| a == "--output-schema"));
            assert!(!review.args.iter().any(|a| a == "/review-local"));
            assert!(review.stdin.as_deref().unwrap().contains("matching the schema"));
        }
        assert_eq!(flag(codex[0], "--sandbox").as_deref(), Some("read-only"));
        assert_eq!(flag(codex[2], "--sandbox").as_deref(), Some("workspace-write"));
        // each review keeps its own answer file
        let out = |j: &Job| flag(j, "-o").unwrap();
        assert!(out(codex[0]).ends_with("codex-reviewLocal.json"));
        assert!(out(codex[1]).ends_with("codex-r1.json"));
        assert!(out(codex[2]).ends_with("codex-finalReview.json"));
        let push_job = codex[3];
        assert!(push_job.args.iter().any(|a| a == "--ephemeral"));
        assert!(push_job.stdin.as_deref().unwrap().contains("(#279)"));
        assert!(
            d.log
                .iter()
                .any(|l| l.text == "Bắt đầu Tự review (Codex · prompt review của app, codex exec chỉ đọc)"),
            "{:?}",
            d.log
        );
        // nothing kept, so nothing to open in a terminal
        assert_eq!(
            resume_command(StepKind::PushSummary, AgentKind::Codex, Some("t-9")),
            None
        );
        assert!(resume_command(StepKind::Fix, AgentKind::Codex, Some("t-9")).is_some());
    }

    #[test]
    fn a_fix_by_another_ai_than_the_coder_starts_fresh_with_the_plan() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, codex_codes());
        let config = Config::from_value(&serde_json::json!({
            "steps": {"autoAdvance": true, "implement": {"agent": "codex"}}
        }));
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        run(&flow, &ctx, FlowAction::ApprovePlan);
        pick_all(&flow, &ctx);
        run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        let jobs = fake.jobs.lock().unwrap().clone();
        // Claude's analysis, then Codex wrote the code without that session
        let implement = jobs.iter().find(|j| j.agent == Provider::Codex).unwrap();
        assert!(!implement.args.iter().any(|a| a == "resume"));
        assert!(implement.stdin.as_deref().unwrap().contains("<brief>"));
        // Claude fixes code it did not write: no --resume, the plan and the brief in the prompt
        let fix = jobs.iter().rev().find(|j| kind(j) == "code").unwrap();
        assert_eq!(fix.agent, Provider::Claude);
        assert_eq!(flag(fix, "--resume"), None);
        let prompt = fix.stdin.as_deref().unwrap();
        assert!(
            prompt.contains("written by another AI") && prompt.contains("<plan>"),
            "{prompt}"
        );
        assert!(prompt.contains("Do not chain commands"));
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

    /// Plays the Kiểm tra commands.
    struct FakeChecks<F: Fn(usize, &checks::Check) -> Result<(Option<i32>, String), String> + Send + Sync> {
        calls: AtomicUsize,
        answer: F,
    }

    impl<F: Fn(usize, &checks::Check) -> Result<(Option<i32>, String), String> + Send + Sync> CheckRunner
        for FakeChecks<F>
    {
        fn run(
            &self,
            _repo: &Path,
            check: &checks::Check,
            _t: Duration,
            _s: &AtomicBool,
        ) -> Result<(Option<i32>, String), String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            (self.answer)(n, check)
        }
    }

    fn with_checks(commands: &[&str]) -> Config {
        let mut config = Config::default();
        config.repos.insert(
            String::new(),
            RepoConfig {
                checks: commands.iter().map(|c| (*c).to_owned()).collect(),
                ..RepoConfig::default()
            },
        );
        config
    }

    #[test]
    fn checks_run_after_coding_and_claude_fixes_what_they_report() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, happy(HIGH));
        flow.set_check_runner(Arc::new(FakeChecks {
            calls: AtomicUsize::new(0),
            answer: |n, check: &checks::Check| {
                assert_eq!(check.args, vec!["eslint", "a.ts"]);
                Ok(if n == 0 {
                    (
                        Some(1),
                        "a.ts\n  1:1  error  'x' is unused  no-unused-vars\n".to_owned(),
                    )
                } else {
                    (Some(0), String::new())
                })
            },
        }));
        let mut config = with_checks(&["npx eslint {files}"]);
        let repo_key = s.repo.to_string_lossy().into_owned();
        let section = config.repos.remove("").unwrap();
        config.repos.insert(repo_key, section);
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        // Viết code, then Kiểm tra by itself (no tokens): one error, Claude fixes it, clean
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(
            (d.current, d.status),
            (Some(StepKind::ReviewLocal), FlowStatus::Ready),
            "{:?}",
            d.error
        );
        let checks: Vec<&RunSummary> = d.runs.iter().filter(|r| r.step == StepKind::Checks).collect();
        assert_eq!(checks.len(), 3, "{:?}", d.runs);
        assert_eq!(checks[0].state, RunState::Failed);
        assert!(checks[0].note.as_deref().unwrap().starts_with("1 lỗi"));
        assert_eq!(checks[1].agent, AgentKind::Claude);
        assert_eq!(
            checks[2].note.as_deref(),
            Some("0 lỗi (npx eslint {files}) sau 1 lần AI sửa")
        );
        let step = d.steps.iter().find(|v| v.kind == StepKind::Checks).unwrap();
        assert_eq!(step.state, StepState::Done);
        let jobs = fake.jobs.lock().unwrap().clone();
        let fix = jobs[2].stdin.as_deref().unwrap();
        assert!(
            fix.contains("<check_output>\n### npx eslint {files}\na.ts: 1:1  error  'x' is unused"),
            "{fix}"
        );
        // the code session goes on
        assert!(
            jobs[2].args.windows(2).any(|w| w == ["--resume", "s-1"]),
            "{:?}",
            jobs[2].args
        );
        assert!(s.tasks.join("WP-1").join("checks").join("checks-1.log").is_file());

        // a Sửa lỗi checks again inside its own step
        run(&flow, &ctx, FlowAction::Run);
        pick_all(&flow, &ctx);
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        let fix = d.runs.iter().rev().find(|r| r.step == StepKind::Fix).unwrap();
        assert!(
            fix.note
                .as_deref()
                .unwrap()
                .ends_with("; Kiểm tra lại: 0 lỗi (npx eslint {files})"),
            "{:?}",
            fix.note
        );
        assert!(
            !d.runs
                .iter()
                .any(|r| r.step == StepKind::Checks && r.started_at > fix.started_at)
        );
    }

    /// Harness: the AI builds it outside the repo, the app runs it; a FAIL goes through the
    /// findings gate, the fix is followed by the app running it again (no tokens), a PASS keeps
    /// it in the repo's library, and every later Sửa lỗi runs it again too.
    #[test]
    fn the_harness_is_built_once_then_run_by_the_app_after_each_fix() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| match kind(job) {
            "analyze" => {
                out(&result_line(
                    "## Nguyên nhân\nx\n```json\n{\"harness\":{\"recommend\":true,\"reason\":\"có logic\",\"checks\":[\"org rỗng\"]}}\n```",
                ));
                ok()
            }
            "harness" => {
                let at = job.args.iter().position(|a| a == "--add-dir").unwrap();
                let scratch = PathBuf::from(&job.args[at + 1]);
                std::fs::write(scratch.join("run.js"), "// harness").unwrap();
                out(&result_line(
                    "Đã dựng.\n```json\n{\"result\":\"fail\",\"summary\":\"2 ca\",\"run\":[\"node\",\"run.js\"],\
                         \"cases\":[{\"name\":\"empty org\",\"ok\":false,\"file\":\"a.ts\"}],\
                         \"negativeControl\":{\"ok\":true,\"detail\":\"bản hỏng FAIL\"},\
                         \"library\":{\"save\":true,\"name\":\"org guard\",\"summary\":\"org rỗng\",\"files\":[\"run.js\"]}}\n```",
                ));
                ok()
            }
            _ => (happy(CLEAN))(job, stop, out),
        });
        let runs = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = runs.clone();
        flow.set_check_runner(Arc::new(FakeChecks {
            calls: AtomicUsize::new(0),
            answer: move |n, check: &checks::Check| {
                assert_eq!(check.kind, CheckKind::Harness);
                assert_eq!(
                    (check.program.as_str(), check.args.clone()),
                    ("node", vec!["run.js".to_owned()])
                );
                seen.lock().unwrap().push(check.dir.clone());
                Ok(if n == 0 {
                    (Some(1), "PASS: owner\nFAIL: empty org — TypeError\n".to_owned())
                } else {
                    (Some(0), "PASS: owner\nPASS: empty org\n".to_owned())
                })
            },
        }));
        let mut config = Config::default();
        let library = s.tasks.parent().unwrap().join("web-harness");
        config.repos.insert(
            s.repo.to_string_lossy().into_owned(),
            RepoConfig {
                harness_dir: Some(library.to_string_lossy().into_owned()),
                ..RepoConfig::default()
            },
        );
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(d.current, Some(StepKind::Harness));

        // built by the AI, run by the app: one case FAILs → the findings gate
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::Findings),
            "{:?}",
            d.error
        );
        assert_eq!(d.current, Some(StepKind::Harness));
        let test = &d.tests[0];
        assert!(!test.passed && test.runs == 1);
        assert_eq!(test.command.as_deref(), Some("node run.js"));
        assert_eq!(test.negative, Some(true));
        assert_eq!(test.cases[0].name, "empty org");
        assert!(test.library.is_none(), "kept only once it passes");
        let f = d
            .findings
            .iter()
            .find(|f| f.title == "Harness FAIL: empty org")
            .unwrap();
        assert_eq!((f.file.as_str(), f.source.as_str()), ("a.ts", "harness"));
        let note = d
            .runs
            .iter()
            .find(|r| r.step == StepKind::Harness)
            .and_then(|r| r.note.clone());
        assert_eq!(note.as_deref(), Some("FAIL 1/2"));
        let jobs = fake.jobs.lock().unwrap().clone();
        let prompt = jobs.last().unwrap().stdin.clone().unwrap();
        assert!(prompt.contains(&library.to_string_lossy().into_owned()) && prompt.contains("- org rỗng"));
        let scratch = PathBuf::from(&runs.lock().unwrap()[0]);
        assert!(!scratch.starts_with(&s.repo) && scratch.join("run.js").is_file());
        assert!(library.join(harness::README).is_file());
        assert!(s.tasks.join("WP-1").join("harness").join("run-1.log").is_file());

        // fixed in step 7, then the app runs the harness again by itself: PASS, kept
        pick_all(&flow, &ctx);
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        assert_eq!(
            (d.current, d.status),
            (Some(StepKind::ReviewLocal), FlowStatus::Ready),
            "{:?}",
            d.error
        );
        let test = &d.tests[0];
        assert!(test.passed && test.runs == 2);
        let kept = PathBuf::from(test.library.clone().unwrap());
        assert_eq!(kept, library.join("org-guard"));
        assert!(kept.join("run.js").is_file());
        let harness_runs: Vec<&RunSummary> = d.runs.iter().filter(|r| r.step == StepKind::Harness).collect();
        assert_eq!(harness_runs.len(), 1, "the app's own run adds no AI run");
        assert!(
            d.findings
                .iter()
                .all(|f| f.state == opw_domain::finding::FindingState::Fixed)
        );
        assert_eq!(
            fake.jobs
                .lock()
                .unwrap()
                .iter()
                .filter(|j| kind(j) == "harness")
                .count(),
            1
        );

        // a fix of what a review found runs the harness again too
        run(&flow, &ctx, FlowAction::Run);
        pick_all(&flow, &ctx);
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        let fix = d.runs.iter().rev().find(|r| r.step == StepKind::Fix).unwrap();
        assert!(
            fix.note.as_deref().unwrap().ends_with("; Harness lại: PASS 2/2"),
            "{:?}",
            fix.note
        );
        assert_eq!(d.tests[0].runs, 3);

        // "Bỏ khỏi thư viện"
        flow.act(ctx.clone(), FlowAction::DropHarness).unwrap();
        assert!(!kept.exists());
        assert!(flow.detail(&ctx).tests[0].library.is_none());
        assert_eq!(
            flow.act(ctx.clone(), FlowAction::DropHarness).unwrap_err().code,
            "harness.library"
        );
    }

    /// Test trình duyệt: Chrome DevTools MCP only, screenshots in the task folder; NG cases go
    /// through the findings gate, are fixed in step 9, and "Không" at "Review chéo lại?" runs the
    /// test again.
    /// A dev server that answers once started, and the test's Chrome window; counts starts and
    /// stops of each.
    #[derive(Default)]
    struct FakeStarter {
        started: Mutex<Vec<(PathBuf, String)>>,
        stops: Arc<AtomicUsize>,
        browsers: Mutex<Vec<(PathBuf, String)>>,
        chrome_stops: Arc<AtomicUsize>,
    }

    struct FakeApp(Arc<AtomicUsize>);

    impl devserver::AppProcess for FakeApp {
        fn exited(&mut self) -> Option<Option<i32>> {
            None
        }
        fn stop(self: Box<Self>) -> Option<String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Some("đã tắt 2 tiến trình: node.exe (cổng 5173)".into())
        }
    }

    impl AppStarter for FakeStarter {
        fn is_up(&self, _url: &str) -> bool {
            !self.started.lock().unwrap().is_empty() && self.stops.load(Ordering::SeqCst) == 0
        }
        fn start(
            &self,
            cwd: &Path,
            command: &str,
            _log: &Path,
        ) -> Result<Box<dyn devserver::AppProcess>, String> {
            self.started
                .lock()
                .unwrap()
                .push((cwd.to_path_buf(), command.to_owned()));
            Ok(Box::new(FakeApp(self.stops.clone())))
        }
        fn open_browser(
            &self,
            profile: &Path,
            url: &str,
        ) -> Result<(Box<dyn devserver::AppProcess>, u16), String> {
            self.browsers
                .lock()
                .unwrap()
                .push((profile.to_path_buf(), url.to_owned()));
            Ok((Box::new(FakeApp(self.chrome_stops.clone())), 9333))
        }
    }

    #[test]
    fn the_app_starts_the_confirmed_dev_server_and_the_ai_only_drives_chrome() {
        let Some(s) = setup() else { return };
        std::fs::create_dir_all(s.repo.join("web")).unwrap();
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| match kind(job) {
            "analyze" => {
                out(&result_line(
                    "## Nguyên nhân\nx\n```json\n{\"browserTest\":{\"recommend\":true,\"reason\":\"đổi màn hình\",\"scenario\":[\"mở /files\"],\
                     \"devServer\":{\"command\":\"npm run dev\",\"dir\":\"web\",\"url\":\"http://localhost:5173\"}}}\n```",
                ));
                ok()
            }
            "browser" => {
                out(&result_line(
                    "```json\n{\"result\":\"ok\",\"summary\":\"ổn\",\"url\":\"http://localhost:5173/files\",\"cases\":[{\"name\":\"mở /files\",\"ok\":true}]}\n```",
                ));
                ok()
            }
            _ => (happy(CLEAN))(job, stop, out),
        });
        let starter = Arc::new(FakeStarter::default());
        flow.set_app_starter(starter.clone());
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        // the analysis only proposes it: nothing is used until the user confirms
        let proposed = d
            .proposals
            .iter()
            .find(|p| p.step == StepKind::BrowserTest)
            .and_then(|p| p.dev_server.clone())
            .unwrap();
        assert_eq!(
            (proposed.command.as_str(), proposed.dir.as_str()),
            ("npm run dev", "web")
        );
        assert_eq!(d.dev_server, None);
        let set = |command: &str, url: &str, dir: &str| FlowAction::SetDevServer {
            command: command.into(),
            url: url.into(),
            dir: dir.into(),
        };
        assert_eq!(
            flow.act(ctx.clone(), set("npm run dev", "localhost:5173", "web"))
                .unwrap_err()
                .code,
            "devserver.invalid"
        );
        assert_eq!(
            flow.act(ctx.clone(), set("npm run dev", "http://localhost:5173", "../x"))
                .unwrap_err()
                .code,
            "devserver.invalid"
        );
        flow.act(ctx.clone(), set(" npm run dev ", "http://localhost:5173", "web/"))
            .unwrap();
        let d = flow.detail(&ctx);
        assert_eq!(d.dev_server, Some(proposed.clone()));
        assert!(!d.dev_server_from_repo);

        let mut d = run(&flow, &ctx, FlowAction::ApprovePlan);
        for _ in 0..12 {
            if d.status == FlowStatus::AwaitingPush || d.current == Some(StepKind::FinalReview) {
                break;
            }
            let action = match (d.status, d.gate.as_ref().map(|g| g.kind)) {
                (FlowStatus::Gate, Some(GateKind::Findings)) => FlowAction::PickFindings { fix: false },
                (FlowStatus::Ready, _) => FlowAction::Run,
                other => panic!("{other:?}: {:?}", d.error),
            };
            d = run(&flow, &ctx, action);
        }
        assert_eq!(
            *starter.started.lock().unwrap(),
            [(s.repo.join("web"), "npm run dev".to_owned())]
        );
        assert_eq!(starter.stops.load(Ordering::SeqCst), 1);
        // the test's own Chrome window, opened on the app and closed with it
        assert_eq!(starter.browsers.lock().unwrap()[0].1, "http://localhost:5173");
        assert_eq!(starter.chrome_stops.load(Ordering::SeqCst), 1);
        let mcp = std::fs::read_to_string(ctx.task_dir.join("browser-test").join("mcp.json")).unwrap();
        assert!(
            mcp.contains("http://127.0.0.1:9333") && !mcp.contains("--headless"),
            "{mcp}"
        );
        let jobs = fake.jobs.lock().unwrap().clone();
        let browser = jobs.iter().find(|j| kind(j) == "browser").unwrap();
        // no shell for the AI: the app runs the dev server
        assert!(
            browser
                .args
                .windows(2)
                .any(|w| w == ["--tools", "Read,Grep,Glob"])
        );
        let prompt = browser.stdin.as_deref().unwrap();
        assert!(
            prompt.contains("already running at http://localhost:5173"),
            "{prompt}"
        );
        let run_row = d.runs.iter().find(|r| r.step == StepKind::BrowserTest).unwrap();
        assert!(
            run_row
                .note
                .as_deref()
                .is_some_and(|n| n.ends_with("app bật rồi tắt dev server")),
            "{:?}",
            run_row.note
        );
        assert!(
            d.log
                .iter()
                .any(|l| l.text.starts_with("Dev server: app chạy `npm run dev`"))
        );
        assert!(
            d.log.iter().any(|l| l
                .text
                .contains("Đã tắt cửa sổ Chrome test và dev server (đã tắt 2 tiến trình")),
            "{:?}",
            d.log
        );

        // an empty URL clears it: the AI starts the app itself again
        flow.act(ctx.clone(), set("", "", "")).unwrap();
        assert_eq!(flow.detail(&ctx).dev_server, None);
        // one remembered for the repo is used when the task has none
        let mut config = Config::default();
        config.repos.insert(
            s.repo.to_string_lossy().into_owned(),
            opw_config::RepoConfig {
                dev_server: Some(proposed.clone()),
                ..opw_config::RepoConfig::default()
            },
        );
        let d = flow.detail(&s.ctx(1, &config));
        assert_eq!(d.dev_server, Some(proposed));
        assert!(d.dev_server_from_repo);
    }

    #[test]
    fn the_ai_asks_for_a_login_and_goes_on_in_the_same_window_after_the_user_answers() {
        let Some(s) = setup() else { return };
        let tests = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| match kind(job) {
            "analyze" => {
                out(&result_line(
                    "## Nguyên nhân\nx\n```json\n{\"browserTest\":{\"recommend\":true,\"scenario\":[\"mở /files\"]}}\n```",
                ));
                ok()
            }
            "browser" => {
                out(&init_line());
                if tests.fetch_add(1, Ordering::SeqCst) == 0 {
                    out(&result_line(
                        "Trang đòi đăng nhập.\n```json\n{\"result\":\"ask\",\"question\":\"Đăng nhập tài khoản test trong cửa sổ Chrome test\",\"url\":\"http://localhost:5173/login\"}\n```",
                    ));
                } else {
                    out(&result_line(
                        "```json\n{\"result\":\"ok\",\"summary\":\"ổn\",\"cases\":[{\"name\":\"mở /files\",\"ok\":true}]}\n```",
                    ));
                }
                ok()
            }
            _ => (happy(CLEAN))(job, stop, out),
        });
        let starter = Arc::new(FakeStarter::default());
        flow.set_app_starter(starter.clone());
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let mut d = run(&flow, &ctx, FlowAction::Run);
        for _ in 0..12 {
            if d.status == FlowStatus::Gate
                && d.gate.as_ref().map(|g| g.kind) == Some(GateKind::AgentQuestion)
            {
                break;
            }
            let action = match (d.status, d.gate.as_ref().map(|g| g.kind)) {
                (FlowStatus::Gate, Some(GateKind::PlanReview)) => FlowAction::ApprovePlan,
                (FlowStatus::Gate, Some(GateKind::Findings)) => FlowAction::PickFindings { fix: false },
                (FlowStatus::Ready, _) => FlowAction::Run,
                other => panic!("{other:?}: {:?}", d.error),
            };
            d = run(&flow, &ctx, action);
        }
        // the AI waits for the user; its window stays open meanwhile
        let q = d.agent_question.clone().expect("a question");
        assert_eq!(q.question, "Đăng nhập tài khoản test trong cửa sổ Chrome test");
        assert_eq!(q.url.as_deref(), Some("http://localhost:5173/login"));
        assert!(q.browser_open);
        assert_eq!(starter.chrome_stops.load(Ordering::SeqCst), 0);
        // no dev server set: the window starts blank and the AI starts the app itself
        assert_eq!(starter.browsers.lock().unwrap().len(), 1);
        assert_eq!(starter.browsers.lock().unwrap()[0].1, "");
        assert!(
            d.steps
                .iter()
                .any(|st| st.kind == StepKind::BrowserTest && st.state == StepState::WaitingUser)
        );
        // nothing else is accepted at this gate
        assert_eq!(
            flow.act(ctx.clone(), FlowAction::ApprovePlan).unwrap_err().code,
            "flow.notNow"
        );

        let d = run(&flow, &ctx, FlowAction::AnswerAgent { answer: " ".into() });
        assert_eq!(d.agent_question, None);
        assert_ne!(d.current, Some(StepKind::BrowserTest), "{:?}", d.error);
        // the same window, the same session, then everything closed
        assert_eq!(starter.browsers.lock().unwrap().len(), 1);
        assert_eq!(starter.chrome_stops.load(Ordering::SeqCst), 1);
        let jobs = fake.jobs.lock().unwrap().clone();
        let tests: Vec<&Job> = jobs.iter().filter(|j| kind(j) == "browser").collect();
        assert_eq!(tests.len(), 2);
        assert!(!tests[0].args.contains(&"--resume".to_owned()));
        assert_eq!(&tests[1].args[1..3], ["--resume", "s-1"]);
        let again = tests[1].stdin.as_deref().unwrap();
        assert!(
            again.starts_with("The user did what you asked.") && again.contains("still open"),
            "{again}"
        );
        assert!(
            d.log
                .iter()
                .any(|l| l.text == "Bạn đã làm việc AI hỏi: AI chạy tiếp")
        );
        let run_rows: Vec<_> = d
            .runs
            .iter()
            .filter(|r| r.step == StepKind::BrowserTest)
            .collect();
        assert!(
            run_rows[0]
                .note
                .as_deref()
                .is_some_and(|n| n.starts_with("AI hỏi: Đăng nhập"))
        );
    }

    #[test]
    fn the_browser_test_reports_ng_cases_with_screenshots_and_runs_again_after_the_fix() {
        let Some(s) = setup() else { return };
        let tests = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| match kind(job) {
            "analyze" => {
                out(&result_line(
                    "## Nguyên nhân\nx\n```json\n{\"browserTest\":{\"recommend\":true,\"reason\":\"đổi màn hình\",\"scenario\":[\"mở /files khi chưa có org\"]}}\n```",
                ));
                ok()
            }
            "browser" => {
                let shots = job
                    .cwd
                    .parent()
                    .unwrap()
                    .join("tasks")
                    .join("WP-1")
                    .join("browser-test");
                let shot = shots.join("1.png");
                std::fs::write(&shot, b"png").unwrap();
                let shot = shot.to_string_lossy().replace('\\', "\\\\");
                let n = tests.fetch_add(1, Ordering::SeqCst);
                let first_ok = if n == 0 { "false" } else { "true" };
                out(&result_line(&format!(
                    "```json\n{{\"result\":\"ng\",\"summary\":\"thử 2 ca\",\"url\":\"http://localhost:5173/files\",\"cases\":[\
                     {{\"name\":\"chưa có org\",\"ok\":{first_ok},\"detail\":\"trang trắng\",\"screenshot\":\"{shot}\",\"file\":\"a.ts\"}},\
                     {{\"name\":\"có org\",\"ok\":true}}]}}\n```"
                )));
                ok()
            }
            _ => (happy(CLEAN))(job, stop, out),
        });
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let mut d = run(&flow, &ctx, FlowAction::Run);
        // the plain answers until the browser test is next
        for _ in 0..12 {
            if d.current == Some(StepKind::BrowserTest) {
                break;
            }
            let action = match (d.status, d.gate.as_ref().map(|g| g.kind)) {
                (FlowStatus::Gate, Some(GateKind::PlanReview)) => FlowAction::ApprovePlan,
                (FlowStatus::Gate, Some(GateKind::Findings)) => FlowAction::PickFindings { fix: false },
                (FlowStatus::Ready, _) => FlowAction::Run,
                other => panic!("{other:?}: {:?}", d.error),
            };
            d = run(&flow, &ctx, action);
        }
        assert_eq!(d.current, Some(StepKind::BrowserTest), "{:?}", d.error);
        // Test trình duyệt comes before Review chốt now
        assert!(
            d.steps
                .iter()
                .any(|s| s.kind == StepKind::FinalReview && s.state == StepState::Pending)
        );
        // "Không sửa" at Review chéo's gate already started it
        let d = if d.status == FlowStatus::Ready {
            run(&flow, &ctx, FlowAction::Run)
        } else {
            d
        };
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::Findings),
            "{:?}",
            d.error
        );
        let view = d.tests.iter().find(|t| t.step == StepKind::BrowserTest).unwrap();
        assert!(!view.passed && view.runs == 1);
        assert_eq!(view.url.as_deref(), Some("http://localhost:5173/files"));
        assert!(
            view.cases[0]
                .screenshot
                .as_deref()
                .is_some_and(|p| p.ends_with("1.png"))
        );
        let f = d
            .findings
            .iter()
            .find(|f| f.title == "Test trình duyệt NG: chưa có org")
            .unwrap();
        assert_eq!((f.file.as_str(), f.source.as_str()), ("a.ts", "browserTest"));
        let note = d
            .runs
            .iter()
            .rev()
            .find(|r| r.step == StepKind::BrowserTest)
            .and_then(|r| r.note.clone());
        assert_eq!(note.as_deref(), Some("NG 1/2, 1 ảnh"));
        let jobs = fake.jobs.lock().unwrap().clone();
        let job = jobs.iter().find(|j| kind(j) == "browser").unwrap();
        let at = job.args.iter().position(|a| a == "--mcp-config").unwrap();
        let mcp = std::fs::read_to_string(&job.args[at + 1]).unwrap();
        assert!(mcp.contains("chrome-devtools-mcp@"));
        assert!(
            job.stdin
                .as_deref()
                .unwrap()
                .contains("Scenario:\n- mở /files khi chưa có org")
        );

        // fixed in step 9; "Không" at "Review chéo lại?" runs the browser test again: OK
        pick_all(&flow, &ctx);
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        assert_eq!(d.gate.as_ref().and_then(|g| g.next), Some(StepKind::BrowserTest));
        let d = run(
            &flow,
            &ctx,
            FlowAction::AfterFix {
                choice: AfterFix::Next,
            },
        );
        // answering the gate starts the browser test at once; this time it is OK, and Review
        // chốt (step 11) comes next to look at the fix too
        assert_eq!(
            (d.current, d.status),
            (Some(StepKind::FinalReview), FlowStatus::Ready),
            "{:?}",
            d.error
        );
        assert_eq!(
            d.steps
                .iter()
                .find(|s| s.kind == StepKind::BrowserTest)
                .map(|s| s.index),
            Some(10)
        );
        let view = d.tests.iter().find(|t| t.step == StepKind::BrowserTest).unwrap();
        assert!(view.passed && view.runs == 2);
        assert!(
            d.findings
                .iter()
                .filter(|f| f.source == "browserTest")
                .all(|f| f.state == opw_domain::finding::FindingState::Fixed)
        );
    }

    fn quota_at(used: f64) -> QuotaSnapshot {
        quota_seen(used, "2026-11-17T08:00:00Z")
    }

    /// A gauge reading taken at `observed_at`.
    fn quota_seen(used: f64, observed_at: &str) -> QuotaSnapshot {
        QuotaSnapshot {
            provider: Provider::Claude,
            windows: vec![opw_domain::view::QuotaWindow {
                kind: opw_domain::view::QuotaWindowKind::FiveHour,
                used_pct: used,
                resets_at: Some("2099-11-17T10:00:00Z".into()),
            }],
            source: "probe".into(),
            observed_at: observed_at.into(),
            plan: None,
        }
    }

    /// The usage limit ends Viết code half way: the step says so with the reset time, and Thử
    /// lại goes on in the cut run's session from where the code is. The reset is announced once.
    #[test]
    fn a_usage_limit_mid_step_waits_for_the_reset_and_goes_on_where_it_stopped() {
        let Some(s) = setup() else { return };
        let codes = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| {
            let attempt = if kind(job) == "code" {
                codes.fetch_add(1, Ordering::SeqCst)
            } else {
                usize::MAX
            };
            if attempt == 0 {
                out(
                    r#"{"type":"system","subtype":"init","session_id":"s-cut","model":"m","tools":["Edit"]}"#,
                );
                out(
                    r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude AI usage limit reached|4070908800","session_id":"s-cut","total_cost_usd":0.4}"#,
                );
                return RunOutcome {
                    code: Some(1),
                    ..RunOutcome::default()
                };
            }
            if attempt == 1 {
                return RunOutcome {
                    code: Some(1),
                    stderr_tail: "429 Too Many Requests".into(),
                    ..RunOutcome::default()
                };
            }
            (happy(CLEAN))(job, stop, out)
        });
        flow.set_quota_source(|_| Some(quota_at(0.0)));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        let err = d.error.clone().unwrap();
        assert_eq!(err.code, "quota.hit", "{}", err.message);
        assert_eq!(err.provider, Some(Provider::Claude));
        assert_eq!(err.resets_at.as_deref(), Some("2099-01-01T00:00:00Z"));
        assert!(
            err.message.contains("Hết hạn mức Claude giữa bước Viết code"),
            "{}",
            err.message
        );
        assert!(err.message.contains("từ chỗ code đang dở"));
        assert_eq!(d.current, Some(StepKind::Implement));

        // the reset time passed: announced once, the task follows the default (no Tự chạy)
        assert_eq!(flow.quota_waits_due("2098-12-31T23:00:00Z", 95.0), vec![]);
        assert_eq!(
            flow.quota_waits_due("2099-01-01T00:00:00Z", 95.0),
            vec![QuotaWake {
                wp_id: 1,
                auto: None,
                retry: true
            }]
        );
        assert_eq!(flow.quota_waits_due("2099-01-01T00:00:05Z", 95.0), vec![]);

        // A restart and a retry failing before init must both keep the interrupted session.
        let flow = Flow::new(s.store.clone(), fake.clone(), None);
        assert_eq!(
            run(&flow, &ctx, FlowAction::Retry).error.unwrap().code,
            "quota.hit"
        );
        // Thử lại: the cut session goes on, told to finish what is left
        let d = run(&flow, &ctx, FlowAction::Retry);
        assert!(d.error.is_none(), "{:?}", d.error);
        let jobs = fake.jobs.lock().unwrap().clone();
        for again in jobs.iter().filter(|j| kind(j) == "code").skip(1) {
            assert!(
                again.args.windows(2).any(|w| w == ["--resume", "s-cut"]),
                "{:?}",
                again.args
            );
            assert!(
                again
                    .stdin
                    .as_deref()
                    .unwrap()
                    .contains("cut off by the usage limit")
            );
        }
    }

    /// "Tự chạy" goes on past the warning level and stops before a step that costs tokens at the
    /// highest one, and says why.
    #[test]
    fn auto_run_stops_before_an_ai_step_when_the_quota_is_almost_gone() {
        let Some(s) = setup() else { return };
        let (flow, fake) = flow_with(&s, false, happy(CLEAN));
        let used = Arc::new(Mutex::new(85.0));
        let level = used.clone();
        flow.set_quota_source(move |_| Some(quota_at(*level.lock().unwrap())));
        let alerts = Arc::new(Mutex::new(Vec::<FlowAlert>::new()));
        let bell = alerts.clone();
        flow.set_alert_sink(move |a| bell.lock().unwrap().push(a));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        flow.act(ctx.clone(), FlowAction::SetAuto { on: true }).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        // 85%: past the warning, Tự chạy goes on from Viết code through Kiểm tra to Tự review
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::Findings),
            "{:?}",
            d.error
        );
        assert!(
            d.log
                .iter()
                .any(|l| l.text == "Claude 5 giờ đã dùng 85%: Tự chạy vẫn chạy tiếp")
        );
        // 96%: a step the user starts still runs, but Tự chạy stops before the next AI step
        *used.lock().unwrap() = 96.0;
        let two = s.ctx(2, &Config::default());
        flow.started(&two, None).unwrap();
        flow.act(two.clone(), FlowAction::SetAuto { on: true }).unwrap();
        run(&flow, &two, FlowAction::Run);
        let d = run(&flow, &two, FlowAction::ApprovePlan);
        assert_eq!(
            (d.current, d.status),
            (Some(StepKind::ReviewLocal), FlowStatus::Ready),
            "{:?}",
            d.error
        );
        assert!(
            d.log.iter().any(|l| l
                .text
                .starts_with("Tự chạy dừng trước Tự review: Claude 5 giờ đã dùng 96%")),
            "{:?}",
            d.log
        );
        let last = alerts.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.title, "WP-2 · Tự chạy dừng vì hạn mức");

        // The pre-launch wait survives a restart. Parking the task prevents any automatic run.
        let flow = Flow::new(s.store.clone(), fake, None);
        let level = used.clone();
        flow.set_quota_source(move |_| Some(quota_at(*level.lock().unwrap())));
        assert!(flow.quota_waits_due("2099-11-17T09:00:00Z", 95.0).is_empty());
        let mut parked = two.row.clone();
        parked.column = "inbox".into();
        s.store.lock().unwrap().upsert_task(&parked).unwrap();
        assert!(flow.quota_waits_due("2099-11-17T10:00:00Z", 95.0).is_empty());
        s.store.lock().unwrap().upsert_task(&two.row).unwrap();
        let due = flow.quota_waits_due("2099-11-17T10:00:00Z", 95.0);
        assert_eq!(
            due,
            vec![QuotaWake {
                wp_id: 2,
                auto: Some(true),
                retry: false
            }]
        );
        assert!(flow.quota_waits_due("2099-11-17T10:00:01Z", 95.0).is_empty());
        *used.lock().unwrap() = 0.0;
        let d = run(&flow, &two, FlowAction::Run);
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);
    }

    /// The cut session cannot be opened again: the app drops it once, and Thử lại runs the step
    /// without it, told that part of the change may already be in the tree.
    #[test]
    fn a_cut_session_the_cli_lost_is_dropped_instead_of_retried_forever() {
        let Some(s) = setup() else { return };
        let codes = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| {
            let attempt = if kind(job) == "code" {
                codes.fetch_add(1, Ordering::SeqCst)
            } else {
                usize::MAX
            };
            if attempt == 0 {
                out(
                    r#"{"type":"system","subtype":"init","session_id":"s-gone","model":"m","tools":["Edit"]}"#,
                );
                out(
                    r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude AI usage limit reached|4070908800","session_id":"s-gone"}"#,
                );
                return RunOutcome {
                    code: Some(1),
                    ..RunOutcome::default()
                };
            }
            if attempt == 1 {
                return RunOutcome {
                    code: Some(1),
                    stderr_tail: "No conversation found with session ID: s-gone".into(),
                    ..RunOutcome::default()
                };
            }
            (happy(CLEAN))(job, stop, out)
        });
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        assert_eq!(
            run(&flow, &ctx, FlowAction::ApprovePlan).error.unwrap().code,
            "quota.hit"
        );
        let d = run(&flow, &ctx, FlowAction::Retry);
        let err = d.error.unwrap();
        assert_eq!(err.code, "session.lost", "{}", err.message);
        assert_eq!(d.current, Some(StepKind::Implement));
        let d = run(&flow, &ctx, FlowAction::Retry);
        assert!(d.error.is_none(), "{:?}", d.error);
        let jobs = fake.jobs.lock().unwrap().clone();
        let codes: Vec<_> = jobs.iter().filter(|j| kind(j) == "code").collect();
        assert!(codes[1].args.windows(2).any(|w| w == ["--resume", "s-gone"]));
        let last = codes[2];
        assert!(!last.args.iter().any(|a| a == "s-gone"), "{:?}", last.args);
        assert!(last.stdin.as_deref().unwrap().contains("its session is gone"));
    }

    #[test]
    fn codex_quota_failure_retries_the_same_step_and_thread() {
        let Some(s) = setup() else { return };
        let calls = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| {
            if job.agent == Provider::Codex && calls.fetch_add(1, Ordering::SeqCst) == 0 {
                out(r#"{"type":"thread.started","thread_id":"cut-thread"}"#);
                out(r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit"}}"#);
                return RunOutcome {
                    code: Some(1),
                    ..RunOutcome::default()
                };
            }
            (happy(CLEAN))(job, stop, out)
        });
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        run(&flow, &ctx, FlowAction::ApprovePlan);
        run(&flow, &ctx, FlowAction::Run); // Tự review
        run(&flow, &ctx, FlowAction::PickFindings { fix: false }); // Review chéo
        let d = settle(&flow, &ctx);
        assert_eq!(d.error.as_ref().map(|e| e.code.as_str()), Some("quota.hit"));
        assert_eq!(d.error.as_ref().unwrap().provider, Some(Provider::Codex));
        assert_eq!(flow.card(&ctx).unwrap().login, None);
        assert!(flow.quota_waits_due("2099-11-17T12:00:00Z", 95.0).is_empty());
        // The CLI gave no reset time: a gauge read before the cut (stale, with room) must not
        // start it again, nor a fresh one in the first minutes.
        flow.set_quota_source(|p| (p == Provider::Codex).then(|| quota_at(40.0)));
        assert!(flow.quota_waits_due("2099-11-17T12:00:00Z", 95.0).is_empty());
        let cut_at = now();
        let soon = opw_quota::unix_to_rfc3339(
            opw_quota::seconds_between("1970-01-01T00:00:00Z", &cut_at).unwrap() + 60.0,
        )
        .unwrap();
        let fresh = soon.clone();
        flow.set_quota_source(move |p| (p == Provider::Codex).then(|| quota_seen(40.0, &fresh)));
        assert!(flow.quota_waits_due(&soon, 95.0).is_empty());
        assert_eq!(
            flow.quota_waits_due("2099-11-17T12:00:00Z", 95.0),
            vec![QuotaWake {
                wp_id: 1,
                auto: None,
                retry: true
            }]
        );
        let d = run(&flow, &ctx, FlowAction::Retry);
        assert!(d.error.is_none(), "{:?}", d.error);
        assert_eq!(d.current, Some(StepKind::FinalReview));
        let jobs = fake.jobs.lock().unwrap();
        let again = jobs.iter().filter(|j| j.agent == Provider::Codex).nth(1).unwrap();
        assert!(again.args.windows(2).any(|w| w == ["--sandbox", "read-only"]));
        assert!(again.args.windows(3).any(|w| w == ["resume", "cut-thread", "-"]));
    }

    #[test]
    fn reset_runs_auto_tasks_and_notifies_manual_tasks_once() {
        for auto in [false, true] {
            let Some(s) = setup() else { return };
            let codes = AtomicUsize::new(0);
            let (flow, fake) = flow_with(&s, false, move |job, stop, out| {
                if kind(job) == "code" && codes.fetch_add(1, Ordering::SeqCst) == 0 {
                    out(
                        r#"{"type":"result","is_error":true,"result":"Claude AI usage limit reached|4070908800","session_id":"cut-session"}"#,
                    );
                    return RunOutcome {
                        code: Some(1),
                        ..RunOutcome::default()
                    };
                }
                (happy(CLEAN))(job, stop, out)
            });
            let config = Config::default();
            let ctx = s.ctx(1, &config);
            flow.started(&ctx, None).unwrap();
            flow.act(ctx.clone(), FlowAction::SetAuto { on: auto }).unwrap();
            let plan = run(&flow, &ctx, FlowAction::Run);
            assert_eq!(quotagate::note_step(&plan), Some(StepKind::Implement));
            let d = run(&flow, &ctx, FlowAction::ApprovePlan);
            assert_eq!(d.error.unwrap().code, "quota.hit");
            let svc =
                crate::AppService::with_flow(s.store.clone(), flow.clone(), config.clone(), s.tasks.clone());
            svc.wake_after_quota(&config, "2099-01-01T00:00:00Z");
            let d = settle(&flow, &ctx);
            if auto {
                assert_eq!(d.gate.unwrap().kind, GateKind::Findings);
                assert_eq!(
                    quotagate::note_step(&flow.detail(&ctx)),
                    Some(StepKind::CrossReview)
                );
                pick_all(&flow, &ctx);
                assert_eq!(quotagate::note_step(&flow.detail(&ctx)), Some(StepKind::Fix));
                assert_eq!(
                    fake.jobs
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|j| kind(j) == "code")
                        .count(),
                    2
                );
            } else {
                assert_eq!(d.error.unwrap().code, "quota.hit");
                assert_eq!(svc.alerts().len(), 1);
                svc.wake_after_quota(&config, "2099-01-01T00:00:05Z");
                assert_eq!(svc.alerts().len(), 1);
                assert_eq!(
                    fake.jobs
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|j| kind(j) == "code")
                        .count(),
                    1
                );
            }
        }
    }

    #[test]
    fn checks_that_cannot_run_stop_the_step() {
        let Some(s) = setup() else { return };
        let (flow, _fake) = flow_with(&s, false, happy(CLEAN));
        flow.set_check_runner(Arc::new(FakeChecks {
            calls: AtomicUsize::new(0),
            answer: |_, _: &checks::Check| Err("không thấy `npx` trên PATH".to_owned()),
        }));
        let mut config = with_checks(&["npx tsc --noEmit"]);
        let section = config.repos.remove("").unwrap();
        config
            .repos
            .insert(s.repo.to_string_lossy().into_owned(), section);
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        run(&flow, &ctx, FlowAction::Run);
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(d.gate.map(|g| g.kind), Some(GateKind::Error));
        let err = d.error.unwrap();
        assert_eq!(err.code, "checks.failed");
        assert!(
            err.message
                .contains("Không chạy được npx tsc --noEmit: không thấy `npx`"),
            "{}",
            err.message
        );
        assert_eq!(d.current, Some(StepKind::Checks));
    }

    #[test]
    fn plan_questions_and_step_advice_wait_for_the_user() {
        let Some(s) = setup() else { return };
        let analyses = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, _stop, out| {
            if kind(job) == "analyze" {
                let n = analyses.fetch_add(1, Ordering::SeqCst);
                let json = if n == 0 {
                    r#"{"questions":[{"text":"Giữ API cũ?","options":["Có","Không"]},{"text":"Thêm log?"}],
                       "harness":{"recommend":false,"reason":"ít logic"},
                       "browserTest":{"recommend":true,"reason":"đổi màn hình","scenario":["mở trang","bấm Lưu"]}}"#
                } else {
                    r#"{"questions":[{"text":"Giữ API cũ?"}],
                       "harness":{"recommend":true,"reason":"có hàm tính","checks":"tổng rỗng"},
                       "browserTest":{"recommend":true,"reason":"đổi màn hình"}}"#
                };
                out(&result_line(&format!(
                    "## Nguyên nhân\nx\n## Rủi ro\nthấp\n\n```json\n{json}\n```"
                )));
                return ok();
            }
            (happy(CLEAN))(job, _stop, out)
        });
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.plan.as_deref(), Some("## Nguyên nhân\nx\n## Rủi ro\nthấp\n"));
        assert_eq!(d.questions.len(), 2);
        assert_eq!(d.questions[0].options, vec!["Có", "Không"]);
        assert_eq!(d.proposals.len(), 2);
        assert_eq!(d.proposals[1].checks, "- mở trang\n- bấm Lưu");
        // the AI's advice decides until the user switches a step
        assert_eq!(d.uses, vec![StepKind::BrowserTest]);
        let analyze = d.runs.iter().find(|r| r.step == StepKind::Analyze).unwrap();
        assert_eq!(
            analyze.note.as_deref(),
            Some("plan: 2 câu hỏi, đề xuất Test trình duyệt")
        );

        let set = |step, on| FlowAction::SetUses { step, on };
        assert_eq!(
            flow.act(ctx.clone(), set(StepKind::Checks, true))
                .unwrap_err()
                .code,
            "flow.uses"
        );
        flow.act(ctx.clone(), set(StepKind::BrowserTest, false)).unwrap();
        let ai = FlowAction::AnswerQuestion {
            id: "q1".into(),
            answer: None,
            ai_decides: true,
        };
        flow.act(ctx.clone(), ai).unwrap();
        let missing = FlowAction::AnswerQuestion {
            id: "q9".into(),
            answer: Some("x".into()),
            ai_decides: false,
        };
        assert_eq!(flow.act(ctx.clone(), missing).unwrap_err().code, "plan.question");
        let err = flow.act(ctx.clone(), FlowAction::ApprovePlan).unwrap_err();
        assert!(err.message.contains("Còn 1 câu hỏi"), "{}", err.message);

        // asking for changes: the answers go into the new analysis and stay on the same question
        let d = run(
            &flow,
            &ctx,
            FlowAction::RejectPlan {
                feedback: "ngắn hơn".into(),
            },
        );
        let jobs = fake.jobs.lock().unwrap().clone();
        let again = jobs[1].stdin.as_deref().unwrap();
        assert!(
            again.contains("<answers>\n- Giữ API cũ?\n  → AI tự quyết\n</answers>"),
            "{again}"
        );
        assert!(!again.contains("Thêm log?\n  →"));
        assert_eq!(d.questions.len(), 1);
        assert!(d.questions[0].ai_decides);
        // the user's "off" holds; Harness follows the new advice
        assert_eq!(d.uses, vec![StepKind::Harness]);

        // approved: Viết code, Kiểm tra (nothing to check here), then Harness waits for Chạy
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(
            (d.current, d.status),
            (Some(StepKind::Harness), FlowStatus::Ready),
            "{:?}",
            d.error
        );
        assert!(
            d.plan
                .as_deref()
                .unwrap()
                .ends_with("- Giữ API cũ?\n  → AI tự quyết\n")
        );
        assert_eq!(
            flow.act(ctx.clone(), set(StepKind::Harness, false))
                .unwrap_err()
                .code,
            "flow.uses"
        );
        // Test trình duyệt is not reached yet: the user can still turn it back on
        flow.act(ctx.clone(), set(StepKind::BrowserTest, true)).unwrap();
        assert_eq!(
            flow.detail(&ctx).uses,
            vec![StepKind::Harness, StepKind::BrowserTest]
        );
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

        let d = run(&flow, &ctx, FlowAction::Run);
        // the plan asks one question (markdown, no json block): approving waits for the answer
        let [q] = &d.questions[..] else {
            panic!("{:?}", d.questions)
        };
        assert_eq!((q.id.as_str(), q.text.as_str()), ("q1", "Có cần migrate không?"));
        assert!(d.proposals.is_empty() && d.uses.is_empty());
        let err = flow.act(ctx.clone(), FlowAction::ApprovePlan).unwrap_err();
        assert_eq!(err.code, "plan.questions");
        let answer = |text: &str| FlowAction::AnswerQuestion {
            id: "q1".into(),
            answer: Some(text.into()),
            ai_decides: false,
        };
        flow.act(ctx.clone(), answer("  ")).unwrap();
        assert!(!flow.detail(&ctx).questions[0].answered());
        flow.act(ctx.clone(), answer("Không, giữ schema")).unwrap();
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(d.current, Some(StepKind::ReviewLocal), "{:?}", d.error);
        // the answer is in plan.md and reaches Viết code through it
        let plan = d.plan.as_deref().unwrap();
        assert!(plan.ends_with("## Câu hỏi và trả lời\n\n- Có cần migrate không?\n  → Không, giữ schema\n"));
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
        assert!(code.contains("Follow them") && code.contains("→ Không, giữ schema"));

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
        assert_eq!(
            plan_note("## Nguyên nhân\nkhông có danh sách", 0, &[]),
            "đã viết plan.md"
        );
        let advice = [
            StepProposal {
                step: StepKind::Harness,
                recommend: true,
                reason: String::new(),
                checks: String::new(),
                dev_server: None,
            },
            StepProposal {
                step: StepKind::BrowserTest,
                recommend: false,
                reason: String::new(),
                checks: String::new(),
                dev_server: None,
            },
        ];
        assert_eq!(plan_note("x", 2, &advice), "plan: 2 câu hỏi, đề xuất Harness");
        // the answers section is replaced, not added twice
        let q = |text: &str, answer: Option<&str>, ai: bool| PlanQuestion {
            id: "q1".into(),
            text: text.into(),
            options: vec![],
            answer: answer.map(str::to_owned),
            ai_decides: ai,
        };
        let once = plan_with_answers("## Rủi ro\nx\n", &[q("A?", Some(" Có "), false)]);
        assert_eq!(once, "## Rủi ro\nx\n\n## Câu hỏi và trả lời\n\n- A?\n  → Có\n");
        let twice = plan_with_answers(&once, &[q("A?", None, true)]);
        assert_eq!(
            twice,
            "## Rủi ro\nx\n\n## Câu hỏi và trả lời\n\n- A?\n  → AI tự quyết\n"
        );
        let kept = new_questions(
            vec![
                text::RawQuestion {
                    text: "B?".into(),
                    options: vec![],
                },
                text::RawQuestion {
                    text: " A? ".into(),
                    options: vec![],
                },
            ],
            &[q("A?", None, true)],
        );
        assert_eq!((kept[0].id.as_str(), kept[0].answered()), ("q1", false));
        assert_eq!((kept[1].id.as_str(), kept[1].ai_decides), ("q2", true));
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
