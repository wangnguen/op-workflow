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
use crate::flow_mcp::{self, Outcome};
use crate::harness;
use crate::quotagate;
use crate::sessions::{Sessions, StepMessage, StepReply};
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
    Decision, DevCommand, DevServer, FindingDecision, FindingView, FlowAction, FlowError, FlowStatus,
    GateKind, GateView, LogLine, Provider, PushBlock, PushView, QuotaSnapshot, ReviewNote, RunState,
    RunSummary, RunView, SessionStatus, StepProposal, StepRef, StepView, TalkTurn, TaskColumn, TaskDetail,
    TestCase, TestView, ToolEvent,
};
use opw_domain::workflow::{AfterFix, Phase, StepKind, StepState, Workflow, WorkflowEvent};
use opw_storage::{FindingRow, StepRunEnd, StepRunRow, Store, TaskRow};
use opw_vcs::Git;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PLAN_FILE: &str = "plan.md";
pub const PUSH_FILE: &str = "push.md";
pub const BRIEF_FILE: &str = "brief.md";
const REVIEW_DIR: &str = "review";
const HARNESS_REVIEW: &str = "harnessReview";
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
const NO_HARNESS_REASON: &str =
    "Phân tích chưa xác định được ca kiểm tra độc lập có kết quả mong đợi rõ ràng.";
/// Times Claude may fix what Kiểm tra reports before the step stops for the user.
const CHECK_FIX_ROUNDS: usize = 2;

// ---------- launching AI processes ----------

/// One AI process to run.
#[derive(Debug, Clone)]
pub struct Job {
    pub agent: Provider,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: Option<String>,
    pub timeout: Duration,
    mcp_token: Option<flow_mcp::Credential>,
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
        let mut env = opw_vcs::guard_env(&self.hooks_dir);
        if let Some(secret) = &job.mcp_token {
            env.push((flow_mcp::TOKEN_ENV.into(), secret.0.clone()));
        }
        let spec = RunSpec {
            program,
            args: job.args.clone(),
            cwd: job.cwd.clone(),
            stdin: job.stdin.clone(),
            env,
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
    workspace: bool,
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
    /// Exact contents verified at the plan gate. Replaced when approved answers are appended.
    #[serde(skip_serializing_if = "Option::is_none")]
    verified_plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approved_brief: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approved_plan: Option<String>,
    /// Exact brief last written by Cập nhật brief, to distinguish edits inside its comments.
    #[serde(skip_serializing_if = "Option::is_none")]
    refreshed_brief: Option<String>,
    /// When the phase last changed.
    since: Option<String>,
    /// A process was running when this was saved.
    in_flight: bool,
    /// The user said the push is done.
    finished: bool,
    /// What the user wrote with "Làm tiếp → quay lại Viết code"; goes into the next Viết code.
    note: Option<String>,
    /// The latest analysis's advice on Harness / Test trình duyệt.
    proposals: Vec<StepProposal>,
    /// Optional steps the user turned on or off by hand: a new analysis no longer changes them.
    picked: Vec<StepKind>,
    /// "Tôi code xong" asked for the review: run it after Kiểm tra even without "Tự chạy".
    review_after_checks: bool,
    /// The user wrote the code by hand ("Tôi code xong"): no code session for Tự review to go on
    /// in (the session there is the analysis's).
    coded_by_hand: bool,
    /// The change as Tự review last saw it ([`change_mark`]): Review chốt is passed while the
    /// code is still the same.
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewed_change: Option<String>,
    /// What Kiểm tra said the last time it ran (its own step, or again after a Sửa lỗi), for the
    /// push summary's `## Testing`.
    #[serde(skip_serializing_if = "Option::is_none")]
    checks_note: Option<String>,
    /// Harness (step 5): what the AI built and how it ran last.
    harness: Option<HarnessState>,
    /// Findings the user routed to the scratch harness.
    #[serde(skip_serializing_if = "HashSet::is_empty")]
    harness_fix: HashSet<String>,
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
    /// Dừng or Claude's budget cap: the session that "Chạy tiếp" can resume.
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped: Option<StoppedRun>,
    /// "Chạy tiếp": the next run of that step goes on in the stopped session with the user's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    go_on: Option<GoOnRun>,
    /// "Trao đổi với AI" at the error gate of a step.
    #[serde(skip_serializing_if = "Option::is_none")]
    talk: Option<Talk>,
    /// What the user and the AI said about the failure, for the next run of that step (when that
    /// run does not go on in the session the conversation happened in).
    #[serde(skip_serializing_if = "Option::is_none")]
    talk_carry: Option<TalkCarry>,
}

/// A conversation about the failure of `step`, read only. It goes on in a Claude session: the
/// failed step's own when it had one (then "Chạy tiếp" carries on with the conversation in it),
/// else a fresh one. Codex gets a fresh read-only session with the whole conversation each turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Talk {
    step: Option<StepKind>,
    agent: Option<AgentKind>,
    session: Option<String>,
    /// `session` is the failed step's own session.
    in_step: bool,
    turns: Vec<TalkTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TalkCarry {
    step: StepKind,
    text: String,
}

/// A run the user stopped with Dừng, whose session can take the user's words next.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoppedRun {
    step: StepKind,
    agent: AgentKind,
    session: String,
}

/// "Chạy tiếp" asked: the stopped run, and what the user wrote (may be empty).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoOnRun {
    step: StepKind,
    agent: AgentKind,
    session: String,
    note: String,
}

/// Errors after which the run's session is kept for "Chạy tiếp": the user stopped it, the cost
/// cap ended it, or the AI ended its turn without the verdict the app needs (it can be told what
/// to do and go on where it was).
const GO_ON_AFTER: [&str; 7] = [
    "step.stopped",
    "step.budget",
    "mcp.mismatch",
    "mcp.blocked",
    "mcp.needsUser",
    "mcp.missingVerdict",
    "mcp.verdictConflict",
];

/// After Dừng, what the error gate says when the run can go on in its session…
const STOPPED_GO_ON: &str = "Bạn đã dừng bước này. Gõ hướng dẫn cho AI rồi bấm Chạy tiếp (AI làm tiếp trong phiên đó), hoặc Chạy lại từ đầu.";
/// … and when it cannot (no session, or a step that keeps none).
const STOPPED_AGAIN: &str = "Bạn đã dừng bước này. Bấm Chạy lại từ đầu để chạy lại.";

/// Whether a stopped run of `step` by `agent` can take more words in its session: Claude keeps
/// one for every AI step but Push summary; Codex goes on (`exec resume`) only in the steps
/// that write code.
fn can_go_on(step: StepKind, agent: AgentKind) -> bool {
    use StepKind as S;
    match agent {
        AgentKind::Claude => !matches!(step, S::Intake | S::Checks | S::PushSummary),
        AgentKind::Codex => matches!(step, S::Implement | S::Fix | S::FixCross | S::Harness),
        AgentKind::Shell => false,
    }
}

/// Cost cap of one answer in "Trao đổi với AI" (Claude).
const TALK_BUDGET_USD: f64 = 0.5;
/// Time limit of one answer.
const TALK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Log lines of the failed step the AI sees.
const TALK_LOG_LINES: usize = 15;

/// "Chạy tiếp": the conversation about a failed step goes to that step's next run, unless that
/// run goes on in the very session the conversation happened in (after talking in the step's own
/// Claude session).
fn carry_talk(saved: &mut Saved, step: StepKind) {
    let Some(talk) = saved.talk.take() else { return };
    if talk.step != Some(step) || talk.turns.is_empty() || talk.in_step {
        return;
    }
    saved.talk_carry = Some(TalkCarry {
        step,
        text: talk_transcript(&talk.turns),
    });
}

/// Làm lại từ đầu (and a task whose repo changed): what the user set for this task stays (the
/// steps it runs, Tự chạy, the issue, the dev server); everything the earlier attempt produced
/// starts over and no finding stays picked for fixing. The caller opens the new session.
fn start_over(entry: &mut Entry, ctx: &TaskCtx, store: &Mutex<Store>) {
    let old = std::mem::take(&mut entry.saved);
    let mut saved = new_saved();
    for step in StepKind::ALL.into_iter().skip(1) {
        let on = old.workflow.enabled(step);
        if on != saved.workflow.enabled(step) {
            let _ = saved.workflow.set_enabled(step, on);
        }
    }
    saved.workspace = old.workspace;
    saved.auto = old.auto;
    saved.issue = old.issue;
    saved.dev_server = old.dev_server;
    saved.picked = old.picked;
    entry.saved = saved;
    if let Ok(store) = store.lock() {
        for mut f in store.findings(ctx.row.wp_id).unwrap_or_default() {
            if f.chosen && !settled(&f.state) {
                f.chosen = false;
                let _ = store.upsert_finding(&f);
            }
        }
    }
}

/// The error `execute` ends with when the task's checkout is not the one it started in: the app
/// starts the task over in a new session there instead of running the step.
const REPO_MOVED: &str = "repo.moved";

/// Thử lại / Chạy lại từ đầu start clean: what was said about the failure stays out of the run.
fn forget_talk(saved: &mut Saved) {
    saved.talk = None;
    saved.talk_carry = None;
}

fn talk_transcript(turns: &[TalkTurn]) -> String {
    turns
        .iter()
        .map(|t| format!("{}: {}", if t.from_user { "User" } else { "AI" }, t.text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Add `text` to a job's prompt: after the prompt on stdin, or after a `/command` argument.
fn add_to_prompt(job: &mut Job, text: &str) {
    if let Some(prompt) = job.stdin.as_mut() {
        prompt.push_str("\n\n");
        prompt.push_str(text);
    } else if let Some(prompt) = job.args.get_mut(1).filter(|a| a.starts_with('/')) {
        prompt.push(' ');
        prompt.push_str(text);
    }
}

/// One answer of "Trao đổi với AI", and where the conversation goes on.
struct TalkAnswer {
    text: String,
    agent: AgentKind,
    /// The Claude session to go on in next turn.
    session: Option<String>,
    in_step: bool,
    cost: Option<f64>,
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
    builder: Option<AgentKind>,
    reviewed: bool,
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
    /// Real data the AI changed while testing.
    changes: Vec<String>,
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
                    untested: c.untested,
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
            changes: self.changes.clone(),
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
                    untested: false,
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
            changes: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct Running {
    step: StepKind,
    session_step: bool,
    session_id: Option<String>,
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
    /// The AI is answering in "Trao đổi với AI"; Dừng sets the flag.
    talking: Option<Arc<AtomicBool>>,
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
/// (`true`). `None`, or one with no URL (only how to reach the screen) = the AI starts the app
/// itself.
fn dev_server_of(ctx: &TaskCtx, saved: &Saved) -> Option<(DevServer, bool)> {
    let usable = |d: &DevServer| {
        (!d.url.trim().is_empty() || !d.how_to.trim().is_empty()) && devserver::check(d).is_ok()
    };
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

fn uses_main_session(step: StepKind, agent: AgentKind) -> bool {
    matches!(
        step,
        StepKind::Analyze
            | StepKind::ReviewLocal
            | StepKind::Fix
            | StepKind::FixCross
            | StepKind::PushSummary
            | StepKind::Checks
    ) || (step == StepKind::Implement && agent == AgentKind::Claude)
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
pub(crate) fn limits(step: StepKind, config: &Config) -> (f64, Duration, &'static str) {
    let steps = &config.steps;
    let (cfg, budget, minutes, key): (&StepConfig, f64, u32, &str) = match step {
        StepKind::Analyze => (&steps.analyze, 5.0, 20, "analyze"),
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

/// How long the Workspace steps bar trusts git's last answer about the task's code.
const CODE_SEEN_FOR: Duration = Duration::from_secs(5);
const NEEDS_CODE: &str =
    "Chưa có code để kiểm hay review: chạy Viết code, hoặc nhờ Claude sửa trong chat trước.";
const NEEDS_PICK: &str =
    "Chưa có lỗi nào được chọn để sửa: chạy một bước review, rồi chọn lỗi cần sửa (Xem quyết định).";

/// Why a step picked in Workspace cannot run now: a review needs code, a fix needs a picked
/// finding. Phân tích and Viết code (from the brief and the conversation) can always run.
fn step_blocked(step: StepKind, findings: &[FindingRow], code: bool) -> Option<&'static str> {
    use StepKind::*;
    match step {
        Intake | Analyze | Implement => None,
        Fix | FixCross => (!findings.iter().any(to_fix)).then_some(NEEDS_PICK),
        Checks | ReviewLocal | Harness | CrossReview | BrowserTest | FinalReview | PushSummary => {
            (!code).then_some(NEEDS_CODE)
        }
    }
}

/// The branch holds work of its own: a tracked file changed, or a commit since it left `base`.
/// Untracked files do not count (local configs would make every repo look changed).
fn repo_changed(repo: &Path, base: &str) -> bool {
    let git = Git::new(repo);
    if git
        .run(&[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=no",
        ])
        .is_ok_and(|out| !out.trim().is_empty())
    {
        return true;
    }
    git.merge_base(base, "HEAD")
        .ok()
        .and_then(|fork| git.run(&["rev-list", "--count", &format!("{fork}..HEAD")]).ok())
        .and_then(|n| n.trim().parse::<u64>().ok())
        .is_some_and(|n| n > 0)
}

fn finding_view(f: &FindingRow, saved: &Saved) -> FindingView {
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
        fix_harness: f.chosen && (f.source == HARNESS_REVIEW || saved.harness_fix.contains(&f.fingerprint)),
        round: f.round,
        step: f.step.as_deref().and_then(step_from_key),
    }
}

/// The fingerprint of a finding `step` reported. A browser test's NG case goes by its number,
/// since the AI words the same case differently from one run to the next.
fn raw_fingerprint(step: StepKind, r: &RawFinding) -> String {
    if step == StepKind::BrowserTest {
        browsertest::fingerprint_of(r)
    } else {
        fingerprint(&r.file, &r.title)
    }
}

/// Merge what a review reported into the stored findings. Reported again → open (unless the
/// user accepted or deferred it); being fixed and not reported again → fixed, unless it is one
/// of `untested` (a browser test case the AI could not run this time): that one is open again.
/// A harness or browser test reports every failure each run, so its own open finding it does
/// not report again passes now: fixed too.
#[allow(clippy::too_many_arguments)]
fn merge_findings(
    store: &Store,
    wp_id: u64,
    step: StepKind,
    round: u8,
    source: &str,
    raw: &[RawFinding],
    untested: &[String],
    at: &str,
) -> opw_storage::StorageResult<()> {
    let existing: HashMap<String, FindingRow> = store
        .findings(wp_id)?
        .into_iter()
        .map(|f| (f.fingerprint.clone(), f))
        .collect();
    let mut seen = HashSet::new();
    for r in raw {
        let fp = raw_fingerprint(step, r);
        let fp = if source == HARNESS_REVIEW {
            format!("harness-review:{fp}")
        } else {
            fp
        };
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
            old.step.as_deref() == Some(step_key(step)) && old.source == source
        } else {
            !test_step(old.step.as_deref())
        }
    };
    for old in existing.values() {
        let passes_now = test_step(Some(step_key(step))) && old.state == "open";
        if (old.state == "fixing" || passes_now) && ours(old) && !seen.contains(&old.fingerprint) {
            // not tested this time: nobody knows whether the fix worked
            let state = if untested.contains(&old.fingerprint) {
                "open"
            } else {
                "fixed"
            };
            store.upsert_finding(&FindingRow {
                state: state.into(),
                chosen: state == "fixed" && old.chosen,
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
    require_mcp: bool,
    config_path: Mutex<Option<PathBuf>>,
    sessions: Mutex<Option<Sessions>>,
    /// wp_id → when git was last asked whether the repo holds the task's code, and the answer.
    code_seen: Mutex<HashMap<u64, (std::time::Instant, bool)>>,
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
    /// A run inside another step (the automatic repair of Kiểm tra, the review of a harness): the
    /// outer step reports to the app, and what this run gives back is checked by the app itself.
    nested: bool,
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
    manual: bool,
}

/// What a step produced: the event for the state machine, one line about the result (the
/// Tiến trình tab and the log) and what it cost.
struct Done {
    event: WorkflowEvent,
    note: String,
    cost: Option<f64>,
}

impl Flow {
    pub fn set_sessions(&self, sessions: Sessions) {
        *self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sessions);
    }

    pub fn workspace(&self, ctx: &TaskCtx) -> opw_domain::session::WorkspaceFlow {
        use opw_domain::session::{WorkspaceFlow, WorkspaceStep};
        self.ensure(ctx);
        let findings = self.inner.findings(ctx.row.wp_id);
        let code = self.inner.has_code(ctx, CODE_SEEN_FOR);
        let session = self.inner.sessions().and_then(|s| s.task_session(ctx.row.wp_id));
        let tasks = self.inner.lock_tasks();
        let entry = &tasks[&ctx.row.wp_id];
        let running = entry.run.as_ref().map(|r| r.step);
        WorkspaceFlow {
            session,
            enabled: entry.saved.workspace,
            auto: ctx.auto(&entry.saved),
            running,
            session_step: entry.run.as_ref().is_some_and(|r| r.session_step),
            gate: match entry.saved.workflow.phase() {
                Phase::Gate(gate) => Some(gate),
                _ => None,
            },
            error: entry.saved.error.clone(),
            steps: StepKind::ALL
                .into_iter()
                .skip(1)
                .map(|step| WorkspaceStep {
                    step,
                    enabled: entry.saved.workflow.enabled(step),
                    state: entry.saved.workflow.state_of(step),
                    why_not: step_blocked(step, &findings, code).map(str::to_owned),
                })
                .collect(),
        }
    }

    pub fn complete_workspace_step(&self, ctx: &TaskCtx, step: StepKind) -> Result<(), ApiError> {
        let tasks = self.inner.lock_tasks();
        let run = tasks
            .get(&ctx.row.wp_id)
            .and_then(|e| e.run.as_ref())
            .filter(|r| r.step == step && r.session_step)
            .ok_or_else(|| ApiError::new("step.missing", "Bước này chưa gửi vào phiên hoặc đã xong"))?;
        let sessions = self.inner.sessions().ok_or_else(flow_missing)?;
        let id = run
            .session_id
            .as_deref()
            .ok_or_else(|| ApiError::new("step.missing", "Bước này chưa gửi vào phiên"))?;
        sessions.complete_flow_step(id, step)
    }

    pub fn set_config_path(&self, path: PathBuf) {
        *self.inner.config_path.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    }

    /// A line in the task's log (Tiến trình), from outside a step.
    pub fn note(&self, wp_id: u64, text: &str) {
        self.inner.log(wp_id, None, "info", text, true);
    }

    pub fn brief_refreshed(&self, ctx: &TaskCtx) {
        self.inner
            .with_saved(ctx.row.wp_id, |s| s.refreshed_brief = ctx.read(BRIEF_FILE));
        self.observe_plan(ctx);
    }

    /// The brief or plan changed after the plan was approved (an edit, refreshed comments): say
    /// so once in the task's chat and ring the bell. Nothing waits for an answer.
    pub fn observe_plan(&self, ctx: &TaskCtx) {
        if !self.inner.require_mcp {
            return;
        }
        let message = {
            let mut tasks = self.inner.lock_tasks();
            let Some(entry) = tasks.get_mut(&ctx.row.wp_id) else {
                return;
            };
            let message = notice_plan_change(entry, ctx);
            if message.is_some() {
                self.inner.persist(ctx.row.wp_id, entry);
            }
            message
        };
        if let Some(message) = message {
            self.inner.tell(ctx, &message, plan_change_alert(ctx, &message));
        }
    }

    /// Load every saved workflow. A step that was running when the app closed is marked failed
    /// (its process is gone) so the user can retry it.
    pub fn new(store: Arc<Mutex<Store>>, launcher: Arc<dyn Launcher>, runs_dir: Option<PathBuf>) -> Self {
        Self::with_verification(store, launcher, runs_dir, true)
    }

    #[cfg(test)]
    pub(crate) fn new_without_mcp(
        store: Arc<Mutex<Store>>,
        launcher: Arc<dyn Launcher>,
        runs_dir: Option<PathBuf>,
    ) -> Self {
        Self::with_verification(store, launcher, runs_dir, false)
    }

    fn with_verification(
        store: Arc<Mutex<Store>>,
        launcher: Arc<dyn Launcher>,
        runs_dir: Option<PathBuf>,
        require_mcp: bool,
    ) -> Self {
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
                    talking: None,
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
                require_mcp,
                config_path: Mutex::new(None),
                sessions: Mutex::new(None),
                code_seen: Mutex::new(HashMap::new()),
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
        // the task goes back to the Inbox: nothing waits on its test window any more
        self.inner.close_browser(wp_id);
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
        self.observe_plan(&ctx);
        if let FlowAction::Talk { text } = &action {
            if let Some(sessions) = self.inner.sessions() {
                if let Some(id) = sessions.task_session(ctx.row.wp_id) {
                    sessions.send(&id, text, false, &[])?;
                    self.inner.log(
                        ctx.row.wp_id,
                        None,
                        "info",
                        "Đã gửi câu hỏi vào chat Workspace",
                        true,
                    );
                    return Ok(());
                }
            }
            return self.talk(ctx, text);
        }
        let wp_id = ctx.row.wp_id;
        // a step picked in Workspace runs only when there is something for it to work on
        if let FlowAction::RunWorkspaceStep { step } = &action {
            let code = self.inner.has_code(&ctx, Duration::ZERO);
            if let Some(why) = step_blocked(*step, &self.inner.findings(wp_id), code) {
                return Err(ApiError::new("step.notReady", why));
            }
        }
        let claim = {
            let mut tasks = self.inner.lock_tasks();
            let entry = tasks.get_mut(&wp_id).ok_or_else(flow_missing)?;
            let previous = matches!(action, FlowAction::RunWorkspaceStep { .. }).then(|| entry.saved.clone());
            let start = apply_action(entry, &ctx, &action, &self.inner.store, self.inner.require_mcp)?;
            if !start && !matches!(action, FlowAction::Run | FlowAction::Retry) {
                self.inner.persist(wp_id, entry);
            }
            if start {
                match self.inner.claim_in(&mut tasks, &ctx) {
                    Ok((claim, text)) => Some((claim, text)),
                    Err(error) => {
                        if let Some(saved) = previous {
                            if let Some(entry) = tasks.get_mut(&wp_id) {
                                entry.saved = saved;
                            }
                        }
                        return Err(error);
                    }
                }
            } else {
                None
            }
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
        // Làm lại từ đầu goes on in a new session; the old one stays in the list to read
        if matches!(action, FlowAction::StartOver) {
            if let (Some(sessions), Some(repo)) = (self.inner.sessions(), ctx.row.repo.as_deref()) {
                sessions.create(Some(wp_id), repo)?;
            }
            self.inner
                .code_seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&wp_id);
        }
        if let Some((claim, text)) = claim {
            if let Ok(store) = self.inner.store.lock() {
                let _ = store.log_event(&now(), Some(wp_id), "flow.info", Some(&text));
            }
            let inner = self.inner.clone();
            std::thread::Builder::new()
                .name(format!("opw-flow-{wp_id}"))
                .spawn(move || worker(&inner, &ctx, claim))
                .map_err(|e| ApiError::new("flow.thread", e.to_string()))?;
        }
        Ok(())
    }

    /// "Trao đổi với AI" at an error gate: keep the user's message, then let the AI answer on a
    /// thread of its own (read only). The step stays failed; Chạy tiếp / Chạy lại từ đầu decide
    /// what comes next, and what was said goes along.
    fn talk(&self, ctx: TaskCtx, text: &str) -> Result<(), ApiError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ApiError::new("talk.empty", "Gõ điều bạn muốn hỏi AI trước."));
        }
        let wp_id = ctx.row.wp_id;
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut tasks = self.inner.lock_tasks();
            let entry = tasks.get_mut(&wp_id).ok_or_else(flow_missing)?;
            if entry.run.is_some() || entry.talking.is_some() {
                return Err(ApiError::new("talk.busy", "AI đang chạy: chờ xong rồi hỏi tiếp."));
            }
            let step = entry
                .saved
                .workflow
                .failed_step()
                .filter(|_| entry.saved.workflow.phase() == Phase::Gate(GateKind::Error))
                .ok_or_else(|| not_now("trao đổi với AI (chỉ khi một bước đang lỗi)"))?;
            let talk = entry.saved.talk.get_or_insert_with(Talk::default);
            if talk.step != Some(step) {
                *talk = Talk {
                    step: Some(step),
                    ..Talk::default()
                };
            }
            talk.turns.push(TalkTurn {
                from_user: true,
                text: text.to_owned(),
                at: now(),
                cost_usd: None,
            });
            entry.talking = Some(stop.clone());
            self.inner.persist(wp_id, entry);
        }
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("opw-talk-{wp_id}"))
            .spawn(move || {
                let answer = inner.talk_turn(&ctx, &stop);
                let mut tasks = inner.lock_tasks();
                let Some(entry) = tasks.get_mut(&wp_id) else {
                    return;
                };
                entry.talking = None;
                let step = entry.saved.talk.as_ref().and_then(|t| t.step);
                let turn = match answer {
                    Ok(a) => {
                        if let Some(t) = entry.saved.talk.as_mut() {
                            t.agent = Some(a.agent);
                            t.session.clone_from(&a.session);
                            t.in_step = a.in_step;
                        }
                        // the conversation happened in the step's own session: Chạy tiếp goes
                        // on in it, conversation included
                        if let (true, Some(step), Some(session)) = (a.in_step, step, a.session) {
                            if can_go_on(step, AgentKind::Claude) {
                                entry.saved.stopped = Some(StoppedRun {
                                    step,
                                    agent: AgentKind::Claude,
                                    session,
                                });
                            }
                        }
                        TalkTurn {
                            from_user: false,
                            text: a.text,
                            at: now(),
                            cost_usd: a.cost,
                        }
                    }
                    Err(why) => TalkTurn {
                        from_user: false,
                        text: format!("⚠ {why}"),
                        at: now(),
                        cost_usd: None,
                    },
                };
                if let Some(t) = entry.saved.talk.as_mut() {
                    t.turns.push(turn);
                }
                inner.persist(wp_id, entry);
            });
        if let Err(e) = spawned {
            if let Some(entry) = self.inner.lock_tasks().get_mut(&wp_id) {
                entry.talking = None;
            }
            return Err(ApiError::new("flow.thread", e.to_string()));
        }
        Ok(())
    }

    /// Fix / accept / defer / reset one finding.
    pub fn decide(&self, d: &FindingDecision) -> Result<(), ApiError> {
        let mut tasks = self.inner.lock_tasks();
        let entry = tasks.get_mut(&d.wp_id).ok_or_else(flow_missing)?;
        if entry.run.is_some() {
            return Err(ApiError::new(
                "step.running",
                "Chờ bước đang chạy xong rồi chọn lỗi.",
            ));
        }
        let store = self.inner.lock_store()?;
        let mut f = store
            .findings(d.wp_id)
            .map_err(|e| ApiError::new("storage", e.to_string()))?
            .into_iter()
            .find(|f| f.fingerprint == d.fingerprint)
            .ok_or_else(|| ApiError::new("finding.notFound", "Không thấy lỗi này nữa"))?;
        let reason = d.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
        match d.decision {
            Decision::Fix | Decision::FixHarness => {
                if d.decision == Decision::FixHarness && f.source != "harness" && f.source != HARNESS_REVIEW {
                    return Err(ApiError::new("finding.target", "Lỗi này không thuộc Harness."));
                }
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
        let mut saved = entry.saved.clone();
        if d.decision == Decision::FixHarness {
            saved.harness_fix.insert(f.fingerprint.clone());
        } else {
            saved.harness_fix.remove(&f.fingerprint);
        }
        let json = serde_json::to_string(&saved).map_err(|e| ApiError::new("storage", e.to_string()))?;
        store
            .save_finding_decision(&f, &json, &now())
            .map_err(|e| ApiError::new("storage", e.to_string()))?;
        entry.saved = saved;
        Ok(())
    }

    pub fn card(&self, ctx: &TaskCtx) -> Option<CardFlow> {
        self.observe_plan(ctx);
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
            findings: findings.iter().map(|f| finding_view(f, &entry.saved)).collect(),
            gate: gate_view(entry, &findings),
        })
    }

    /// Everything the task window shows. Reads the task files; call it off the UI thread.
    pub fn detail(&self, ctx: &TaskCtx) -> TaskDetail {
        self.ensure(ctx);
        self.observe_plan(ctx);
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
            continuation_budget_usd: entry
                .saved
                .error
                .as_ref()
                .filter(|e| e.code == "step.budget")
                .and_then(|_| current_step(entry))
                .map(|step| limits(ai_limit_step(step), &ctx.config).0),
            // only while the step it is about is the one failed
            talk: entry
                .saved
                .talk
                .as_ref()
                .filter(|t| {
                    entry.saved.workflow.phase() == Phase::Gate(GateKind::Error)
                        && t.step == entry.saved.workflow.failed_step()
                })
                .map(|t| t.turns.clone())
                .unwrap_or_default(),
            talk_busy: entry.talking.is_some(),
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
            can_go_on: entry.saved.stopped.as_ref().is_some_and(|s| {
                entry.saved.workflow.phase() == Phase::Gate(GateKind::Error)
                    && entry.saved.workflow.failed_step() == Some(s.step)
            }),
            uses: SWITCHABLE
                .into_iter()
                .filter(|s| entry.saved.workflow.uses(*s))
                .collect(),
            findings: findings.iter().map(|f| finding_view(f, &entry.saved)).collect(),
            review_notes: review_notes(ctx),
            push,
            log: entry.log.iter().cloned().collect(),
            runs: {
                let allowed = ctx.repo_config().allow_bash;
                let mut list: Vec<RunSummary> = runs
                    .iter()
                    .filter_map(|r| {
                        run_summary(
                            r,
                            entry.run.as_ref(),
                            self.inner.runs_dir.as_deref(),
                            &ctx.row.display_id,
                            &allowed,
                        )
                    })
                    .collect();
                // only the run whose question is still open waits; one the user answered is over
                let asking = entry.saved.workflow.phase() == Phase::Gate(GateKind::AgentQuestion);
                let last = list.len().saturating_sub(1);
                for (i, r) in list.iter_mut().enumerate() {
                    if r.state == RunState::Waiting && !(asking && i == last) {
                        r.state = RunState::Done;
                    }
                }
                list
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
        "waiting" => RunState::Waiting,
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

pub(crate) fn chat_item(i: transcript::Item) -> ChatItem {
    use transcript::ItemKind as K;
    ChatItem {
        kind: match i.kind {
            K::Prompt => ChatKind::Prompt,
            K::User => ChatKind::User,
            K::Text => ChatKind::Text,
            K::Thinking => ChatKind::Thinking,
            K::Tool => ChatKind::Tool,
            K::End => ChatKind::End,
            K::Error => ChatKind::Error,
        },
        step: None,
        text: i.text,
        tool: i.tool,
        output: i.output,
        output_cut: u32::try_from(i.output_cut).unwrap_or(u32::MAX),
        failed: i.failed,
        denied: i.denied,
        children: i.children.into_iter().map(chat_item).collect(),
        running: i.running,
        tool_id: i.tool_id,
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
fn plan_note(plan: &str, proposals: &[StepProposal]) -> String {
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

fn plan_digest(ctx: &TaskCtx, brief: &str, plan: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"opw-plan-audited-v1\0");
    for part in [
        ctx.row.project.as_str(),
        ctx.row.repo.as_deref().unwrap_or(""),
        ctx.row.base.as_deref().unwrap_or(""),
        ctx.row.branch.as_deref().unwrap_or(""),
        brief,
        plan.trim_end(),
    ] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn plan_stamp(ctx: &TaskCtx) -> Option<String> {
    Some(plan_digest(ctx, &ctx.read(BRIEF_FILE)?, &ctx.read(PLAN_FILE)?))
}

fn remember_plan(saved: &mut Saved, ctx: &TaskCtx) -> Result<(), FlowError> {
    let brief = ctx
        .read(BRIEF_FILE)
        .ok_or_else(|| flow_error("brief.missing", "Không đọc được brief.md"))?;
    let plan = ctx
        .read(PLAN_FILE)
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| flow_error("plan.missing", "Không đọc được plan.md"))?;
    saved.verified_plan = Some(plan_digest(ctx, &brief, &plan));
    saved.approved_brief = Some(brief);
    saved.approved_plan = Some(plan);
    saved.refreshed_brief = None;
    Ok(())
}

fn plan_change_message(saved: &Saved, ctx: &TaskCtx) -> String {
    let current = ctx.read(BRIEF_FILE).unwrap_or_default();
    let old = saved.approved_brief.as_deref().unwrap_or_default();
    let comments = |brief: &str| {
        brief
            .split_once("## Comment trên OpenProject")
            .map(|(_, rest)| rest.split("\n## ").next().unwrap_or_default().trim().to_owned())
            .unwrap_or_default()
    };
    let fresh = comments(&current);
    let before = comments(old);
    let added: Vec<_> = fresh
        .split("\n### ")
        .filter(|part| !part.trim().is_empty() && !before.contains(part.trim()))
        .collect();
    let manual = opw_context::replace_comment_section(old, "").trim()
        != opw_context::replace_comment_section(&current, "").trim()
        || saved.approved_plan != ctx.read(PLAN_FILE)
        || (before != fresh
            && saved.refreshed_brief.as_ref().map(|b| comments(b)).as_deref() != Some(fresh.as_str()));
    let mut message = String::from("Brief đổi sau khi bạn duyệt plan.");
    if before != fresh && !added.is_empty() {
        message.push_str("\nComment mới hoặc đã cập nhật trong brief:\n");
        message.push_str(&added.join("\n### "));
    }
    if manual || before == fresh {
        message.push_str("\nBrief / plan được sửa tay.");
    }
    message.push('\n');
    message.push_str(PLAN_CHANGE_HINT);
    message
}

/// How a change of the brief after approval is handled: by the user, in the chat.
const PLAN_CHANGE_HINT: &str = "Muốn AI xem lại thì nhắn trong chat hoặc chạy lại Phân tích.";

/// Called only while idle, including right after a step: what to tell the user when the brief
/// or plan changed since the plan was approved. Once per change: the contents now become the
/// approved ones.
fn notice_plan_change(entry: &mut Entry, ctx: &TaskCtx) -> Option<String> {
    if entry.run.is_some()
        || entry.saved.finished
        || entry.saved.verified_plan.is_none()
        || matches!(
            entry.saved.workflow.phase(),
            Phase::NotStarted | Phase::Running(StepKind::Analyze) | Phase::Gate(GateKind::PlanReview)
        )
    {
        return None;
    }
    let stamp = plan_stamp(ctx);
    if entry.saved.verified_plan == stamp {
        return None;
    }
    let message = plan_change_message(&entry.saved, ctx);
    entry.saved.verified_plan = stamp;
    entry.saved.approved_brief = ctx.read(BRIEF_FILE);
    entry.saved.approved_plan = ctx.read(PLAN_FILE);
    entry.saved.refreshed_brief = None;
    push_log(
        entry,
        current_step(entry),
        "info",
        "Brief đổi sau khi bạn duyệt plan: đã báo trong chat của phiên",
    );
    Some(message)
}

fn plan_change_alert(ctx: &TaskCtx, message: &str) -> FlowAlert {
    FlowAlert {
        wp_id: ctx.row.wp_id,
        task_id: ctx.row.display_id.clone(),
        kind: AlertKind::Comment,
        title: format!("{} · Brief đổi sau khi duyệt plan", ctx.row.display_id),
        body: message.to_owned(),
        tab: "brief",
    }
}

/// Apply a button to the saved state. Returns `true` when a step should start now.
fn apply_action(
    entry: &mut Entry,
    ctx: &TaskCtx,
    action: &FlowAction,
    store: &Mutex<Store>,
    require_mcp: bool,
) -> Result<bool, ApiError> {
    use WorkflowEvent as E;
    let running = entry.run.is_some();
    let phase = entry.saved.workflow.phase();
    if entry.talking.is_some() && !matches!(action, FlowAction::Stop | FlowAction::SetAuto { .. }) {
        return Err(ApiError::new(
            "talk.busy",
            "AI đang trả lời trong khung Trao đổi với AI: chờ xong hoặc bấm Dừng.",
        ));
    }
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
                    entry.saved.stopped = None;
                    entry.saved.go_on = None;
                    forget_talk(&mut entry.saved);
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
            let stopped = entry
                .saved
                .error
                .as_ref()
                .is_some_and(|e| e.code == "step.stopped");
            event(entry, E::Retry(step))?;
            entry.saved.error = None;
            entry.saved.stopped = None;
            entry.saved.go_on = None;
            forget_talk(&mut entry.saved);
            let text = if stopped {
                format!("Chạy lại {} từ đầu", step_label(step))
            } else {
                format!("Thử lại {}", step_label(step))
            };
            push_log(entry, Some(step), "info", &text);
            Ok(true)
        }
        FlowAction::GoOn { note } => {
            if running {
                return Err(ApiError::new("step.running", "Bước này đang chạy"));
            }
            let stopped = entry
                .saved
                .stopped
                .clone()
                .filter(|s| {
                    phase == Phase::Gate(GateKind::Error)
                        && entry.saved.workflow.failed_step() == Some(s.step)
                })
                .ok_or_else(|| not_now("chạy tiếp (phiên của lần chạy này không còn để tiếp)"))?;
            let agent = match &entry.saved.harness {
                Some(h) if stopped.step == StepKind::Harness && !h.reviewed && !h.run.is_empty() => {
                    other_agent(h.builder.unwrap_or_else(|| ctx.agent_for(StepKind::Harness)))
                }
                _ => ctx.agent_for(ai_limit_step(stopped.step)),
            };
            if agent != stopped.agent {
                return Err(ApiError::new(
                    "flow.goOn",
                    "AI của bước này đã đổi trong Cài đặt nên không làm tiếp phiên cũ được: bấm Chạy lại từ đầu",
                ));
            }
            event(entry, E::Retry(stopped.step))?;
            entry.saved.error = None;
            carry_talk(&mut entry.saved, stopped.step);
            let note = note.trim().to_owned();
            let text = if note.is_empty() {
                format!("Chạy tiếp {} trong phiên cũ", step_label(stopped.step))
            } else {
                format!("Chạy tiếp {}: {}", step_label(stopped.step), short(&note))
            };
            push_log(entry, Some(stopped.step), "info", &text);
            entry.saved.go_on = Some(GoOnRun {
                step: stopped.step,
                agent: stopped.agent,
                session: stopped.session,
                note,
            });
            entry.saved.stopped = None;
            Ok(true)
        }
        FlowAction::Stop => {
            if let Some(talking) = &entry.talking {
                talking.store(true, Ordering::Relaxed);
                return Ok(false);
            }
            let run = entry
                .run
                .as_ref()
                .ok_or_else(|| not_now("dừng (không có bước nào đang chạy)"))?;
            run.stop.store(true, Ordering::Relaxed);
            Ok(false)
        }
        // `Flow::act` runs it on its own thread; it never reaches here
        FlowAction::Talk { .. } => Err(not_now("trao đổi với AI")),
        FlowAction::ApprovePlan => {
            if ctx.read(PLAN_FILE).is_none_or(|p| p.trim().is_empty()) {
                return Err(ApiError::new("plan.missing", "Chưa có plan.md để duyệt"));
            }
            if phase != Phase::Gate(GateKind::PlanReview) {
                return Err(not_now("duyệt plan"));
            }
            if require_mcp {
                remember_plan(&mut entry.saved, ctx).map_err(|e| ApiError::new(e.code, e.message))?;
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
            entry.saved.verified_plan = None;
            push_log(
                entry,
                Some(StepKind::Analyze),
                "info",
                "Bạn góp ý plan: phân tích lại",
            );
            Ok(true)
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
        FlowAction::SetDevServer {
            url,
            commands,
            how_to,
        } => {
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
            let server = devserver::tidy(url, commands, how_to);
            devserver::check(&server).map_err(|e| ApiError::new("devserver.invalid", e))?;
            let mut text = if server.url.is_empty() {
                "Bỏ lệnh chạy app: Test trình duyệt để AI tự bật app".to_owned()
            } else if server.commands.is_empty() {
                format!("Test trình duyệt dùng app đang chạy ở {}", server.url)
            } else {
                format!(
                    "Test trình duyệt: app chạy {} rồi mở {}",
                    devserver::what_runs(&server),
                    server.url
                )
            };
            if !server.how_to.is_empty() {
                text.push_str(&format!("; cách vào chỗ test: {}", short(&server.how_to)));
            }
            let keep = !server.url.is_empty() || !server.how_to.is_empty();
            entry.saved.dev_server = keep.then_some(server);
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
        FlowAction::SetWorkspaceFlow { on } => {
            if running {
                return Err(not_now("đổi flow khi có bước đang chạy"));
            }
            entry.saved.workspace = *on;
            if !on {
                entry.saved.auto = Some(false);
            }
            Ok(false)
        }
        FlowAction::SetWorkspaceStep { step, on } => {
            if running || !entry.saved.workspace || entry.saved.finished {
                return Err(not_now("bật/tắt bước"));
            }
            entry
                .saved
                .workflow
                .set_enabled(*step, *on)
                .map_err(|e| ApiError::new("flow.invalid", e.to_string()))?;
            if !entry.saved.picked.contains(step) {
                entry.saved.picked.push(*step);
            }
            Ok(false)
        }
        FlowAction::RunWorkspaceStep { step } => {
            if running || !entry.saved.workspace || entry.saved.finished {
                return Err(not_now("chạy bước"));
            }
            if *step == StepKind::Implement && phase == Phase::Gate(GateKind::PlanReview) {
                return apply_action(entry, ctx, &FlowAction::ApprovePlan, store, require_mcp);
            }
            // at "Chọn lỗi cần sửa", Sửa lỗi fixes what was picked there, for that review
            if step.is_fix() && phase == Phase::Gate(GateKind::Findings) {
                return apply_action(
                    entry,
                    ctx,
                    &FlowAction::PickFindings { fix: true },
                    store,
                    require_mcp,
                );
            }
            entry
                .saved
                .workflow
                .select(*step)
                .map_err(|e| ApiError::new("flow.invalid", e.to_string()))?;
            entry.saved.error = None;
            Ok(true)
        }
        FlowAction::StartOver => {
            if running {
                return Err(ApiError::new("step.running", "Bước này đang chạy"));
            }
            start_over(entry, ctx, store);
            push_log(
                entry,
                None,
                "info",
                "Làm lại từ đầu: phiên mới, bắt đầu từ Phân tích",
            );
            Ok(false)
        }
        FlowAction::SelfCode => {
            if phase != Phase::Gate(GateKind::PlanReview) {
                return Err(not_now("tự code"));
            }
            if require_mcp {
                remember_plan(&mut entry.saved, ctx).map_err(|e| ApiError::new(e.code, e.message))?;
            }
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
            entry.saved.coded_by_hand = true;
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
            if require_mcp && phase != Phase::AwaitingPush {
                return Err(ApiError::new(
                    "flow.notReady",
                    "Task chưa được xác nhận sẵn sàng bàn giao. Hoàn thành các bước đang chờ trước.",
                ));
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
                    entry.saved.verified_plan = None;
                }
                // asked for by hand: Review chốt runs even on code Tự review already saw
                if step == StepKind::FinalReview {
                    entry.saved.reviewed_change = None;
                }
                if let Some(h) = entry.saved.harness.as_mut().filter(|_| step == StepKind::Harness) {
                    h.run.clear();
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

    fn sessions(&self) -> Option<Sessions> {
        self.sessions.lock().ok()?.clone()
    }

    /// A line from the app in the task's chat, when it has a session (never sent to the AI), and
    /// the bell. Call it without the task lock.
    fn tell(&self, ctx: &TaskCtx, line: &str, alert: FlowAlert) {
        if let Some(sessions) = self.sessions() {
            if let Some(id) = sessions.task_session(ctx.row.wp_id) {
                let _ = sessions.note(&id, line);
            }
        }
        if let Some(sink) = self.alert_sink.lock().ok().and_then(|s| s.clone()) {
            sink(alert);
        }
    }

    /// The task has code to check or review: Viết code ran, the user coded by hand, or git sees
    /// a change on the branch (asked again after `max_age`).
    fn has_code(&self, ctx: &TaskCtx, max_age: Duration) -> bool {
        let wp_id = ctx.row.wp_id;
        let wrote = self.lock_tasks().get(&wp_id).is_some_and(|e| {
            e.saved.coded_by_hand || e.saved.workflow.state_of(StepKind::Implement) == StepState::Done
        });
        if wrote {
            return true;
        }
        let cached = self
            .code_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&wp_id)
            .filter(|(at, _)| at.elapsed() < max_age)
            .map(|(_, code)| *code);
        if let Some(code) = cached {
            return code;
        }
        let code = ctx
            .repo()
            .zip(ctx.row.base.as_deref())
            .is_some_and(|(repo, base)| repo_changed(&repo, base));
        self.code_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(wp_id, (std::time::Instant::now(), code));
        code
    }

    /// Steps that run in the task's Workspace session leave its id as the task's Claude session,
    /// so a step with its own `claude -p` (Harness, a cut run) may resume it. Two processes must
    /// not write one transcript while the chat may hold it: that run continues in a fork.
    fn fork_workspace_session(&self, wp_id: u64, job: &mut Job) {
        let Some(workspace) = self.sessions().and_then(|s| s.task_session(wp_id)) else {
            return;
        };
        let resumes = job
            .args
            .windows(2)
            .any(|w| w[0] == "--resume" && w[1] == workspace);
        if resumes && !job.args.iter().any(|a| a == "--fork-session") {
            job.args.push("--fork-session".into());
        }
    }

    /// Claims a step under the tasks lock and returns the log line to keep.
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
        let sessions = self.sessions();
        let session_step = sessions.is_some() && uses_main_session(step, agent) && step != StepKind::Checks;
        // a step sent into the task's session (or Kiểm tra, whose fixes go there) needs it free;
        // Review chéo, Harness, Test trình duyệt… run in their own process and do not wait for it
        if session_step || step == StepKind::Checks {
            if let Some(s) = &sessions {
                if let Some(id) = s.task_session(ctx.row.wp_id) {
                    s.flow_ready(&id)?;
                }
            }
        }
        let agent = if session_step { AgentKind::Claude } else { agent };
        let choice = model_choice(step, &ctx.config);
        let mode = if session_step {
            step_config(step, &ctx.config.steps).and_then(|c| c.permission_mode.clone())
        } else {
            permission_mode(step, &ctx.config).filter(|_| agent != AgentKind::Shell)
        };
        let command = command_label(step, agent, ctx.repo().as_deref());
        let stop = Arc::new(AtomicBool::new(false));
        entry.run = Some(Running {
            step,
            session_step,
            session_id: None,
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
                nested: false,
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
        // the row tells how the step went, not only how the process ended: the AI stopped to ask,
        // or the step failed after a clean exit (an answer the app could not use)
        let row_state = match &result {
            Ok(done) if matches!(done.event, WorkflowEvent::AgentAsked(_)) => Some("waiting"),
            Err(_) => Some("failed"),
            Ok(_) => None,
        };
        if let (Some(id), Some(state), Ok(store)) = (early, row_state, self.store.lock()) {
            let _ = store.set_step_run_state(id, state);
        }
        // Dừng: the run's session, for "Chạy tiếp"
        let stopped_run = match &result {
            Err(e) if GO_ON_AFTER.contains(&e.code.as_str()) && !quitting => {
                early.and_then(|id| self.runs(wp_id).into_iter().find(|r| r.id == id))
            }
            _ => None,
        };
        let mut tasks = self.lock_tasks();
        let entry = tasks.get_mut(&wp_id)?;
        let run_id = entry.run.take().and_then(|r| r.run_id);
        entry.saved.in_flight = false;
        entry.saved.since = Some(now());
        // `note` goes on the run's row (Tiến trình), `text` into the log
        let mut note = String::new();
        let asked = matches!(&result, Ok(done) if matches!(done.event, WorkflowEvent::AgentAsked(_)));
        let mut moved: Option<String> = None;
        let (kind, text) = match result {
            Ok(done) => match entry.saved.workflow.apply(done.event) {
                // the AI stopped to ask: the step is not over, it waits for the user
                Ok(()) if asked => {
                    entry.saved.error = None;
                    note.clone_from(&done.note);
                    let cost = done.cost.map(|c| format!(" ({})", usd(c))).unwrap_or_default();
                    (
                        "info",
                        format!("{} dừng, chờ bạn: {}{cost}", step_label(step), done.note),
                    )
                }
                Ok(()) => {
                    entry.saved.error = None;
                    if entry.saved.cut.as_ref().is_some_and(|c| c.step == step) {
                        entry.saved.cut = None;
                    }
                    if step == StepKind::Analyze {
                        entry.saved.feedback = None;
                    }
                    // the conversation about this step's failure is over
                    entry.saved.talk = None;
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
            // the checkout changed under the task: start over in a new session there
            Err(err) if err.code == REPO_MOVED => {
                start_over(entry, ctx, &self.store);
                let line = format!(
                    "Repo của task đã đổi ({}): app mở phiên mới ở repo hiện tại, bắt đầu lại từ Phân tích. \
                     Phiên cũ vẫn còn trong danh sách để xem.",
                    err.message
                );
                moved = Some(line.clone());
                ("info", line)
            }
            Err(mut err) => {
                let _ = entry.saved.workflow.apply(WorkflowEvent::StepFailed(step));
                // a session kept from an earlier failure is not this run's
                entry.saved.stopped = None;
                if GO_ON_AFTER.contains(&err.code.as_str()) && !quitting {
                    entry.saved.go_on = None;
                    entry.saved.stopped = stopped_run.as_ref().and_then(|row| {
                        let agent = agent_of_row(row);
                        let session = row.session_id.clone().filter(|s| !s.is_empty())?;
                        (can_go_on(step, agent) || (err.code == "step.budget" && agent == AgentKind::Claude))
                            .then_some(StoppedRun { step, agent, session })
                    });
                    if err.code == "step.stopped" {
                        err.message = if entry.saved.stopped.is_some() {
                            STOPPED_GO_ON
                        } else {
                            STOPPED_AGAIN
                        }
                        .to_owned();
                    }
                }
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
            && entry
                .saved
                .harness
                .as_ref()
                .is_some_and(|h| h.reviewed && !h.run.is_empty());
        let changed = if self.require_mcp {
            notice_plan_change(entry, ctx)
        } else {
            None
        };
        let go_on = kind == "ok"
            && match entry.saved.workflow.phase() {
                Phase::Running(StepKind::Checks) => !entry.saved.workspace || ctx.auto(&entry.saved),
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
                        && entry
                            .saved
                            .harness
                            .as_ref()
                            .is_some_and(|h| h.reviewed && !h.run.is_empty())) =>
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
        if alert.is_none() && moved.is_none() {
            alert = (!go_on).then(|| stop_alert(ctx, step, entry, &text)).flatten();
        }
        self.persist(wp_id, entry);
        let next = go_on.then(|| self.claim_in(&mut tasks, ctx));
        drop(tasks);
        if let Some(line) = moved {
            if let (Some(sessions), Some(repo)) = (self.sessions(), ctx.row.repo.as_deref()) {
                let _ = sessions.create(Some(wp_id), repo);
            }
            self.code_seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&wp_id);
            let alert = FlowAlert {
                wp_id,
                task_id: ctx.row.display_id.clone(),
                kind: AlertKind::Gate,
                title: format!("{} · Repo đổi: phiên mới", ctx.row.display_id),
                body: line.clone(),
                tab: "progress",
            };
            self.tell(ctx, &line, alert);
        }
        if let Some(message) = changed {
            self.tell(ctx, &message, plan_change_alert(ctx, &message));
        }
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
    fn open_log(&self, ctx: &TaskCtx, claim: &Claim, job: &Job) -> (Option<std::fs::File>, Option<String>) {
        use std::io::Write;
        let Some(runs) = self.runs_dir.as_ref() else {
            return (None, None);
        };
        let stamp = now().replace([':', '.'], "-");
        let suffix = step_key(claim.step);
        let name = format!("{}/{stamp}-{suffix}.jsonl", ctx.row.display_id);
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

    fn attach_mcp(
        &self,
        ctx: &TaskCtx,
        claim: &Claim,
        job: &mut Job,
    ) -> Result<Option<flow_mcp::Run>, FlowError> {
        if !self.require_mcp
            || !ctx.config.steps.verify_mcp
            || claim.nested
            || flow_mcp::criteria(claim.step).is_empty()
        {
            return Ok(None);
        }
        let rows = self.findings(ctx.row.wp_id);
        let (plan, checks, findings) = self
            .with_saved(ctx.row.wp_id, |s| {
                let plan = (claim.step != StepKind::Analyze)
                    .then(|| ctx.read(PLAN_FILE))
                    .flatten();
                let selected = s.workflow.fixing().is_some();
                let findings: Vec<FindingView> = rows
                    .iter()
                    .filter(|row| {
                        claim.step == StepKind::PushSummary
                            || (selected && claim.step.is_fix() && (row.state == "fixing" || to_fix(row)))
                    })
                    .map(|row| finding_view(row, s))
                    .collect();
                (plan, checked_lines(s), findings)
            })
            .unwrap_or_default();
        let context = serde_json::json!({
            "taskId": ctx.row.display_id,
            "project": ctx.row.project,
            "repo": job.cwd,
            "branch": ctx.row.branch,
            "base": ctx.row.base,
            "step": claim.step,
            "purpose": "executeStep",
            "brief": ctx.read(BRIEF_FILE),
            "approvedPlan": plan,
            "checksActuallyRun": checks,
            "findings": findings,
        });
        let mcp = flow_mcp::Run::start(claim.step, &job.cwd, context).map_err(|e| {
            flow_error(
                "mcp.start",
                format!("Không mở được MCP để xác minh bước: {e}. Bấm Thử lại."),
            )
        })?;
        let instructions = flow_mcp::instructions(claim.step);
        if let Some(prompt) = job.stdin.as_mut() {
            *prompt = format!("{instructions}\n\n{prompt}");
        } else if let Some(prompt) = job.args.get_mut(1).filter(|arg| arg.starts_with('/')) {
            prompt.push(' ');
            prompt.push_str(&instructions);
        } else {
            return Err(flow_error(
                "mcp.prompt",
                "Không gắn được hướng dẫn xác minh vào bước này.",
            ));
        }
        mcp.configure(job.agent, &mut job.args);
        job.mcp_token = Some(flow_mcp::Credential(mcp.token().to_owned()));
        Ok(Some(mcp))
    }

    fn check_mcp(
        &self,
        ctx: &TaskCtx,
        claim: &Claim,
        mcp: Option<flow_mcp::Run>,
        answer: &str,
    ) -> Result<Option<String>, FlowError> {
        let Some(mcp) = mcp else { return Ok(None) };
        let verdict = mcp.finish().ok_or_else(|| flow_error(
            "mcp.missingVerdict",
            "AI đã thoát nhưng chưa nộp kết quả xác minh. Bước chưa hoàn thành. Xem Chat rồi bấm Thử lại.",
        ))?;
        let summary = opw_context::redact(&verdict.summary, &[]);
        let summary = short(&summary);
        let browser_question = claim.step == StepKind::BrowserTest && browsertest::question(answer).is_some();
        match verdict.outcome {
            Outcome::Completed if !browser_question => {
                self.log(
                    ctx.row.wp_id,
                    Some(claim.step),
                    "info",
                    &format!("AI xác nhận đạt tiêu chí: {summary}"),
                    true,
                );
                Ok(verdict.result_text)
            }
            Outcome::NeedsUser if browser_question => Ok(None),
            Outcome::Completed => Err(flow_error(
                "mcp.verdictConflict",
                "AI báo hoàn thành nhưng kết quả trình duyệt vẫn đang hỏi bạn. Bấm Thử lại.",
            )),
            Outcome::Mismatch => Err(flow_error(
                "mcp.mismatch",
                format!("AI phát hiện lệch yêu cầu hoặc repo: {summary}"),
            )),
            Outcome::Blocked => Err(flow_error(
                "mcp.blocked",
                format!("AI chưa thể hoàn thành bước: {summary}"),
            )),
            Outcome::NeedsUser => Err(flow_error(
                "mcp.needsUser",
                format!("AI cần bạn xử lý trước khi chạy lại: {summary}"),
            )),
        }
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
        // A capped push summary can resume too, while Dừng keeps its existing fresh-run choice.
        if step == StepKind::PushSummary {
            job.args.retain(|arg| arg != "--no-session-persistence");
        }
        // "Chạy tiếp" after Dừng: the stopped session, the user's words as its next message
        let go_on = self.take_go_on(ctx.row.wp_id, step, AgentKind::Claude);
        if let Some(g) = go_on {
            if let Some(i) = job.args.iter().position(|a| a == "--resume") {
                let end = (i + 2).min(job.args.len());
                job.args.drain(i..end);
            }
            job.args.extend(["--resume".to_owned(), g.session]);
            if job.args.get(1).is_some_and(|arg| arg.starts_with('/')) {
                job.args.remove(1);
            }
            job.stdin = Some(if step == StepKind::Analyze {
                prompts::go_on_analysis(&g.note)
            } else {
                prompts::go_on(&g.note, None)
            });
        }
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
        if !claim.nested {
            if let Some(text) = self.take_talk_carry(ctx.row.wp_id, step) {
                add_to_prompt(&mut job, &prompts::talk_carry(&text));
            }
        }
        self.fork_workspace_session(ctx.row.wp_id, &mut job);
        let mcp = self.attach_mcp(ctx, claim, &mut job)?;
        let job = &job;
        let stop: &AtomicBool = &claim.stop;
        use std::io::Write;
        let wp_id = ctx.row.wp_id;
        let (budget, timeout, key) = limits(ai_limit_step(step), &ctx.config);
        let (mut raw, log) = self.open_log(ctx, claim, job);
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
                    "Bước này chạm trần ${budget}. Chọn Chạy tiếp để cấp thêm ngân sách trong cùng phiên, hoặc Chạy lại từ đầu. Đổi trần tại Cài đặt → Các bước AI → {}.",
                    step_label(ai_limit_step(step))
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
            Some(mut r) => {
                if let Some(text) = self.check_mcp(ctx, claim, mcp, &r.text)? {
                    r.text = text;
                }
                Ok(r)
            }
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
        // "Chạy tiếp" after Dừng: `exec … resume <stopped thread> -` with the user's words
        let resumed;
        let go_on = self.take_go_on(wp_id, step, AgentKind::Codex);
        let job = match go_on {
            Some(g) => {
                let mut j = job.clone();
                match j.args.iter().position(|a| a == "resume") {
                    Some(i) if i + 1 < j.args.len() => j.args[i + 1] = g.session,
                    _ => j.args = codex::with_resume(j.args, &g.session),
                }
                j.stdin = Some(prompts::go_on(&g.note, None));
                resumed = j;
                &resumed
            }
            None => job,
        };
        let mut job = job.clone();
        if !claim.nested {
            if let Some(text) = self.take_talk_carry(ctx.row.wp_id, step) {
                add_to_prompt(&mut job, &prompts::talk_carry(&text));
            }
        }
        let mcp = self.attach_mcp(ctx, claim, &mut job)?;
        let job = &job;
        let (_, timeout, key) = limits(ai_limit_step(step), &ctx.config);
        let (mut raw, log) = self.open_log(ctx, claim, job);
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
            return Err(self.quota_hit(ctx, step, Provider::Codex, Some(thread.as_str()), &quota_text));
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
        if let Some(text) = self.check_mcp(ctx, claim, mcp, &last)? {
            last = text;
        }
        Ok((last, thread))
    }

    /// Run the step's AI on `job` (built for that AI); text, session and cost come back alike.
    fn run_ai(&self, ctx: &TaskCtx, step: StepKind, job: &Job, claim: &Claim) -> Result<AiRun, FlowError> {
        if uses_main_session(step, claim.agent) && ctx.repo().as_ref() == Some(&job.cwd) {
            if let Some(sessions) = self.sessions() {
                let reply = self.run_main_session(&sessions, ctx, step, job, claim)?;
                return Ok(AiRun {
                    text: reply.result.text,
                    session: reply.result.session_id,
                    cost: Some(reply.result.cost_usd),
                    denials: reply.result.denials.len(),
                    manual: reply.manual,
                });
            }
        }
        if claim.agent == AgentKind::Codex {
            let (text, thread) = self.run_codex(ctx, step, job, claim)?;
            return Ok(AiRun {
                text,
                session: thread,
                cost: None,
                denials: 0,
                manual: false,
            });
        }
        let r = self.run_claude(ctx, step, job, claim)?;
        Ok(AiRun {
            text: r.text,
            session: r.session_id,
            cost: Some(r.cost_usd),
            denials: r.denials.len(),
            manual: false,
        })
    }

    fn run_main_session(
        &self,
        sessions: &Sessions,
        ctx: &TaskCtx,
        step: StepKind,
        job: &Job,
        claim: &Claim,
    ) -> Result<StepReply, FlowError> {
        let repo = ctx
            .row
            .repo
            .as_deref()
            .ok_or_else(|| flow_error("repo.missing", "Task chưa có repo"))?;
        let id = sessions
            .ensure_task(ctx.row.wp_id, repo)
            .map_err(|e| flow_error(&e.code, e.message))?
            .id;
        // "Chạy tiếp" after Dừng: the session already holds the step, only the user's words go
        let go_on = if claim.nested {
            None
        } else {
            self.take_go_on(ctx.row.wp_id, step, AgentKind::Claude)
        };
        let prompt = match &go_on {
            Some(g) if step == StepKind::Analyze => prompts::go_on_analysis(&g.note),
            Some(g) => prompts::go_on(&g.note, None),
            None => job.stdin.clone().unwrap_or_else(|| {
                job.args
                    .windows(2)
                    .find(|p| p[0] == "-p" && p[1].starts_with('/'))
                    .map_or_else(|| prompt_of(job), |p| p[1].clone())
            }),
        };
        let prompt = if claim.nested || prompt.starts_with('/') {
            prompt
        } else {
            format!(
                "{prompt}\n\nThis is the Workspace step \"{}\". When you finish it, look up the op_workflow MCP tools with ToolSearch if needed. Call get_active_step, then complete_step with its returned id and your summary.",
                step_label(step)
            )
        };
        let run_id = self.begin_run(ctx, step, "claude", None);
        if let Some(r) = self
            .lock_tasks()
            .get_mut(&ctx.row.wp_id)
            .and_then(|e| e.run.as_mut())
        {
            r.session_id = Some(id.clone());
        }
        let reply = sessions
            .run_flow_step(
                &id,
                StepMessage {
                    step,
                    prompt: &prompt,
                    mode: claim
                        .mode
                        .as_deref()
                        .filter(|m| opw_domain::session::MODES.contains(m)),
                    model: claim.choice.model.as_deref(),
                    wait_for_report: !claim.nested,
                },
                &claim.stop,
                job.timeout,
            )
            .map_err(|e| flow_error(&e.code, e.message));
        if let (Some(run_id), Ok(store)) = (run_id, self.store.lock()) {
            let r = reply.as_ref().ok().map(|r| &r.result);
            let _ = store.end_step_run(
                run_id,
                &StepRunEnd {
                    state: if reply.is_ok() { "done" } else { "failed" },
                    session_id: Some(&id),
                    ended_at: &now(),
                    cost_usd: r.map(|r| r.cost_usd),
                    tokens_in: r.map(|r| r.tokens_in),
                    tokens_out: r.map(|r| r.tokens_out),
                    ..StepRunEnd::default()
                },
            );
        }
        if let Ok(reply) = &reply {
            self.log(ctx.row.wp_id, Some(step), "info", &reply.summary, true);
        }
        reply
    }

    fn round_of(&self, wp_id: u64) -> u8 {
        self.lock_tasks()
            .get(&wp_id)
            .map(|e| e.saved.workflow.round())
            .unwrap_or_default()
    }

    /// "Chạy tiếp" after Dừng, for this run of `step` by `agent` (taken once).
    /// The conversation about this step's last failure, once, for its next run.
    fn take_talk_carry(&self, wp_id: u64, step: StepKind) -> Option<String> {
        let mut tasks = self.lock_tasks();
        let entry = tasks.get_mut(&wp_id)?;
        let carry = entry.saved.talk_carry.take_if(|c| c.step == step)?;
        self.persist(wp_id, entry);
        Some(carry.text)
    }

    /// One answer of "Trao đổi với AI". Read only: Claude with Read/Grep/Glob, Codex in its
    /// read-only sandbox; the repo is compared before and after. `Err` = why there is no answer.
    fn talk_turn(&self, ctx: &TaskCtx, stop: &AtomicBool) -> Result<TalkAnswer, String> {
        use std::io::Write;
        let wp_id = ctx.row.wp_id;
        let repo = ctx
            .repo()
            .filter(|r| r.is_dir())
            .ok_or("Không thấy thư mục repo của task.")?;
        let (talk, error, log, stopped) = {
            let tasks = self.lock_tasks();
            let entry = tasks.get(&wp_id).ok_or("Task không còn trong luồng.")?;
            let talk = entry.saved.talk.clone().unwrap_or_default();
            let mut log: Vec<String> = entry
                .log
                .iter()
                .rev()
                .filter(|l| l.step.is_some() && l.step == talk.step)
                .take(TALK_LOG_LINES)
                .map(|l| l.text.clone())
                .collect();
            log.reverse();
            (
                talk,
                entry.saved.error.clone(),
                log.join("\n"),
                entry.saved.stopped.clone(),
            )
        };
        let step = talk.step.ok_or("Không rõ bước đang lỗi.")?;
        let question = talk
            .turns
            .last()
            .filter(|t| t.from_user)
            .map(|t| t.text.clone())
            .ok_or("Chưa có câu hỏi.")?;
        let first = talk.agent.is_none();
        // who answers, in which session: the step's own Claude session when it has one
        let (agent, session, in_step) = if first {
            let own = stopped
                .filter(|s| s.step == step && s.agent == AgentKind::Claude)
                .map(|s| s.session)
                .or_else(|| {
                    self.runs(wp_id)
                        .into_iter()
                        .rev()
                        .find(|r| step_from_key(&r.step) == Some(step))
                        .filter(|r| agent_of_row(r) == AgentKind::Claude)
                        .and_then(|r| r.session_id)
                        .filter(|s| !s.is_empty())
                });
            match own {
                Some(s) => (AgentKind::Claude, Some(s), true),
                None if matches!(step, StepKind::Intake | StepKind::Checks) => {
                    (AgentKind::Claude, None, false)
                }
                None => (ctx.agent_for(step), None, false),
            }
        } else {
            (
                talk.agent.unwrap_or(AgentKind::Claude),
                talk.session.clone(),
                talk.in_step,
            )
        };
        let error_text = error
            .as_ref()
            .map(|e| format!("{} ({})", e.message, e.code))
            .unwrap_or_default();
        let prompt = if !first && session.is_some() {
            prompts::talk_followup(&question)
        } else {
            let brief = ctx.read(BRIEF_FILE).unwrap_or_default();
            let plan = ctx.read(PLAN_FILE).unwrap_or_default();
            let status = Git::new(&repo).run(&["status", "--short"]).unwrap_or_default();
            let earlier = talk_transcript(&talk.turns[..talk.turns.len() - 1]);
            let context = prompts::TalkContext {
                brief: &brief,
                plan: &plan,
                status: &status,
                earlier: &earlier,
            };
            prompts::talk(
                step_label(step),
                &error_text,
                &log,
                (!in_step).then_some(&context),
                &question,
            )
        };
        let provider = provider_of(agent);
        let mut args = match provider {
            Provider::Codex => codex::analyze_args(),
            Provider::Claude => claude::analyze_args(TALK_BUDGET_USD, &[]),
        };
        if let (Provider::Claude, Some(s)) = (provider, &session) {
            args.extend(["--resume".to_owned(), s.clone()]);
        }
        let job = Job {
            agent: provider,
            args,
            cwd: repo.clone(),
            stdin: Some(prompt),
            timeout: TALK_TIMEOUT,
            mcp_token: None,
        };
        let mut raw = self.runs_dir.as_ref().and_then(|runs| {
            let stamp = now().replace([':', '.'], "-");
            let path = runs.join(format!("{}/{stamp}-talk.jsonl", ctx.row.display_id));
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::File::create(path).ok()
        });
        let before = git_snap(&repo).ok();
        let mut answer: Option<String> = None;
        let mut failed: Option<String> = None;
        let mut new_session = String::new();
        let mut cost = None;
        let mut over = false;
        let mut on_line = |line: &str| {
            if let Some(f) = raw.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            if provider == Provider::Codex {
                match codex::parse_line(line) {
                    Ok(CodexEvent::Message(m)) => answer = Some(m),
                    Ok(CodexEvent::Failed(m)) => failed = Some(m),
                    _ => {}
                }
                return;
            }
            for event in claude::parse_line(line).unwrap_or_default() {
                match event {
                    ClaudeEvent::Init { session_id, .. } => new_session = session_id,
                    ClaudeEvent::Result(r) => {
                        over = r.over_budget();
                        cost = Some(r.cost_usd);
                        if !r.session_id.is_empty() {
                            new_session.clone_from(&r.session_id);
                        }
                        if r.is_error {
                            failed = Some(r.text.clone());
                        } else {
                            answer = Some(r.text.clone());
                        }
                    }
                    _ => {}
                }
            }
        };
        let outcome = self
            .launcher
            .launch(&job, stop, &mut on_line)
            .map_err(|e| match e {
                LaunchError::Missing => format!("Không tìm thấy {} trên máy.", agent_name(agent)),
                LaunchError::Spawn(why) => format!("Không chạy được {}: {why}", agent_name(agent)),
            })?;
        if stop.load(Ordering::Relaxed) {
            return Err("Bạn đã dừng lượt trả lời này.".into());
        }
        if over {
            return Err(format!(
                "Lượt trả lời chạm trần ${TALK_BUDGET_USD}. Hỏi gọn hơn, hoặc chọn Chạy tiếp / Chạy lại từ đầu."
            ));
        }
        let mut text = match (answer.filter(|a| !a.trim().is_empty()), failed) {
            (Some(a), None) => a,
            (_, Some(why)) if looks_signed_out(&why) => {
                return Err(format!(
                    "{} đã đăng xuất: đăng nhập lại rồi hỏi tiếp.",
                    agent_name(agent)
                ));
            }
            (_, Some(why)) => return Err(format!("AI không trả lời được: {}", short(&why))),
            (None, None) if quotagate::looks_quota_hit(&outcome.stderr_tail) => {
                return Err("Hết hạn mức của AI này: chờ reset rồi hỏi tiếp.".into());
            }
            (None, None) => return Err(format!("AI không trả lời được: {}", exit_why(&outcome))),
        };
        if let (Some(before), Ok(after)) = (before, git_snap(&repo)) {
            if let Some(what) = git_surprise(StepKind::CrossReview, &before, &after) {
                text.push_str(&format!(
                    "\n\n⚠ App thấy {what} trong lượt trao đổi này (lượt này chỉ được đọc). Xem lại repo trước khi chạy tiếp."
                ));
            }
        }
        Ok(TalkAnswer {
            text,
            agent,
            session: (provider == Provider::Claude && !new_session.is_empty()).then_some(new_session),
            in_step,
            cost,
        })
    }

    fn take_go_on(&self, wp_id: u64, step: StepKind, agent: AgentKind) -> Option<GoOnRun> {
        self.with_saved(wp_id, |s| s.go_on.take_if(|g| g.step == step && g.agent == agent))
            .flatten()
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
        self.merge_untested(ctx, step, round, source, raw, &[])
    }

    /// [`Self::merge`] where `untested` cases (fingerprints) ran no test this time.
    fn merge_untested(
        &self,
        ctx: &TaskCtx,
        step: StepKind,
        round: u8,
        source: &str,
        raw: &[RawFinding],
        untested: &[String],
    ) -> Result<Vec<FindingRow>, FlowError> {
        let store = self
            .store
            .lock()
            .map_err(|_| flow_error("storage", "Dữ liệu cục bộ đang bị khoá"))?;
        merge_findings(&store, ctx.row.wp_id, step, round, source, raw, untested, &now())
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
                "Bước chạy quá {} phút nên app đã dừng nó. Đổi Thời gian tối đa tại Cài đặt → Các bước AI → {}, rồi bấm Thử lại.",
                timeout.as_secs() / 60,
                step_from_key(key).map_or("Bước này", step_label)
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
fn review_event(step: StepKind, findings: &[FindingRow], raw: &[RawFinding]) -> (WorkflowEvent, String) {
    let reported: HashSet<String> = raw.iter().map(|r| raw_fingerprint(step, r)).collect();
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

/// Check the repo, run the step, check git again.
fn execute(inner: &Inner, ctx: &TaskCtx, claim: &Claim) -> Result<Done, FlowError> {
    let step = claim.step;
    let repo = ctx.repo().filter(|p| p.is_dir()).ok_or_else(|| {
        flow_error(
            "repo.missing",
            "Không thấy thư mục repo của task (đã xoá hoặc đổi chỗ?)",
        )
    })?;
    if inner.require_mcp {
        let stored = inner
            .lock_store()
            .map_err(|e| flow_error(&e.code, e.message))?
            .kv_get(ctx.row.wp_id, crate::task_repo::KEY, "")
            .map_err(|e| flow_error("storage", e.to_string()))?;
        let saved = stored.and_then(|v| serde_json::from_str::<crate::task_repo::Binding>(&v).ok());
        let binding = match saved {
            Some(binding) => binding,
            // a task started before bindings were kept (or one whose binding cannot be read):
            // bind it to the checkout as it is now
            None => {
                let brief = ctx.read(BRIEF_FILE).unwrap_or_default();
                let binding = crate::task_repo::Binding::read(
                    &Git::new(&repo),
                    crate::task_repo::legacy_issues(&brief),
                )
                .map_err(|e| flow_error(&e.code, e.message))?;
                if let (Ok(json), Ok(store)) = (serde_json::to_string(&binding), inner.lock_store()) {
                    let _ = store.kv_set(ctx.row.wp_id, crate::task_repo::KEY, "", &json, &now());
                }
                binding
            }
        };
        // the folder or origin changed under the task: bind it to the checkout as it is now; the
        // step does not run with a plan made for the old one
        if let Some((now_bound, what)) = binding.moved(&repo).map_err(|e| flow_error(&e.code, e.message))? {
            let json = serde_json::to_string(&now_bound).map_err(|e| flow_error("storage", e.to_string()))?;
            inner
                .lock_store()
                .map_err(|e| flow_error(&e.code, e.message))?
                .kv_set(ctx.row.wp_id, crate::task_repo::KEY, "", &json, &now())
                .map_err(|e| flow_error("storage", e.to_string()))?;
            return Err(flow_error(REPO_MOVED, what));
        }
    }
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
    let after = git_snap(&repo);
    if inner.require_mcp && after.is_err() {
        return Err(flow_error(
            "repo.git",
            "Không đọc được trạng thái Git sau bước chạy. Kiểm tra repo rồi thử lại.",
        ));
    }
    if let Ok(after) = after {
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
    if inner.require_mcp && result.is_ok() && claim.stop.load(Ordering::Relaxed) {
        return Err(flow_error("step.stopped", "Bạn đã dừng bước này."));
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

/// The AI repair inside Kiểm tra uses Sửa lỗi; command timeouts stay separate.
fn ai_limit_step(step: StepKind) -> StepKind {
    if step == StepKind::Checks {
        StepKind::Fix
    } else {
        step
    }
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
    let (_, timeout, _) = limits(StepKind::Checks, &ctx.config);
    let (repair_budget, repair_timeout, _) = limits(StepKind::Fix, &ctx.config);
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
        let lines = checks::ChangedLines::of(repo, base);
        let list = checks::plan(repo, &config.checks, &changed);
        if list.is_empty() {
            return Ok(ChecksDone {
                ran: false,
                note: "Không dò thấy lệnh kiểm tra cho các file đã đổi. Thêm lệnh ở Cài đặt → Repo → Lệnh kiểm tra nếu repo cần."
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
                Ok((code, output)) => {
                    let mut r = checks::read(repo, check, code, &output, &changed, &lines);
                    checks::forgive_old_format(
                        repo,
                        base,
                        check,
                        &mut r,
                        checker.as_ref(),
                        timeout,
                        &claim.stop,
                    );
                    r
                }
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
        // the latest round is where the code stands now (the push summary reports it)
        inner.with_saved(wp_id, |s| s.checks_note = Some(note.clone()));
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
            if !own_step && !inner.require_mcp {
                return Ok(ChecksDone {
                    ran: true,
                    note: format!("{note} (xem tab Tiến trình; chạy lại Kiểm tra sau khi sửa tay)"),
                    cost,
                });
            }
            let message = match broken.first() {
                Some(b) => format!(
                    "Không chạy được {}: {}. Sửa lệnh ở Cài đặt → Repo → Lệnh kiểm tra hoặc cài công cụ còn thiếu, rồi chạy lại Kiểm tra (hỏi AI ở khung Trao đổi với AI nếu chưa rõ).",
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
            agent: if inner.sessions().is_some() {
                AgentKind::Claude
            } else {
                agent_for(StepKind::Fix, &ctx.config)
            },
            stop: claim.stop.clone(),
            choice: model_choice(StepKind::Fix, &ctx.config),
            mode: permission_mode(StepKind::Fix, &ctx.config),
            nested: true,
        };
        let allowed = config.allow_bash.clone();
        let session = inner.with_saved(wp_id, |s| s.session_of(fixer.agent)).flatten();
        let prompt = prompts::fix_checks(&checks::report(&results), shell_of(&fixer, &allowed));
        let job = code_job(
            &fixer,
            session.as_deref(),
            &[],
            repair_budget,
            &allowed,
            repo,
            repair_timeout,
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
        let mut args = if claim.step == StepKind::Harness {
            claude::harness_args(budget, allowed, dirs)
        } else {
            claude::code_args(resume, budget, allowed)
        };
        if claim.step.is_fix() && !dirs.is_empty() {
            args.push("--add-dir".into());
            args.extend(dirs.iter().cloned());
        }
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
        mcp_token: None,
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

/// Review chốt passed because Tự review saw this very code.
const FINAL_REVIEW_KEPT: &str = "bỏ qua: code không đổi từ lúc Tự review xong, giữ kết quả Tự review";

/// FNV-1a 64, fed piece by piece.
fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Untracked files bigger than this count by size and time only (build output nobody ignored).
const MARK_READ_BYTES: u64 = 8 * 1024 * 1024;

/// A short mark of the task's change: the fork point of `base`, the diff of the tracked files
/// and every untracked file's name and bytes. The same mark = the same code. `None` when git
/// cannot tell (then nothing is passed on the strength of it).
fn change_mark(repo: &Path, base: &str) -> Option<String> {
    let git = Git::new(repo);
    let fork = git.merge_base(base, "HEAD").ok()?;
    let diff = git
        .run(&[
            "--no-optional-locks",
            "-c",
            "core.quotepath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--binary",
            &fork,
            "--",
        ])
        .ok()?;
    let listed = git
        .run(&[
            "--no-optional-locks",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .ok()?;
    let mut hash = fnv(
        fnv(fnv(0xcbf2_9ce4_8422_2325, fork.as_bytes()), b"\0"),
        diff.as_bytes(),
    );
    let mut names: Vec<&str> = listed.split('\0').filter(|n| !n.is_empty()).collect();
    names.sort_unstable();
    for name in names {
        hash = fnv(fnv(fnv(hash, b"\0"), name.as_bytes()), b"\0");
        let file = repo.join(name);
        hash = match std::fs::metadata(&file) {
            Ok(m) if m.len() > MARK_READ_BYTES => {
                let at = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                fnv(hash, format!("{}@{at}", m.len()).as_bytes())
            }
            Ok(_) => fnv(hash, &std::fs::read(&file).unwrap_or_default()),
            Err(_) => fnv(hash, b"?"),
        };
    }
    Some(format!("{hash:016x}"))
}

/// After Tự review: remember the change it saw.
fn mark_reviewed(inner: &Inner, wp_id: u64, step: StepKind, repo: &Path, base: &str) {
    if step == StepKind::ReviewLocal {
        let mark = change_mark(repo, base);
        inner.with_saved(wp_id, |s| s.reviewed_change = mark);
    }
}

/// Review chốt can be passed: Tự review saw this very code (any later fix, by any step or by
/// hand, changes the mark).
fn unchanged_since_self_review(inner: &Inner, wp_id: u64, repo: &Path, base: &str) -> bool {
    let seen = inner.with_saved(wp_id, |s| s.reviewed_change.clone()).flatten();
    seen.is_some_and(|seen| change_mark(repo, base).as_deref() == Some(seen.as_str()))
}

/// What the app really ran on the change, one line each, for the push summary's `## Testing`.
fn checked_lines(s: &mut Saved) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(note) = s.checks_note.as_deref().filter(|n| !n.trim().is_empty()) {
        lines.push(format!(
            "Kiểm tra (type / lint / format on the lines the task changed): {note}"
        ));
    }
    let names = |cases: Vec<&str>| cases.join(", ");
    if let Some(h) = s
        .harness
        .as_ref()
        .filter(|h| s.workflow.uses(StepKind::Harness) && h.runs > 0)
    {
        let verdict = if h.passed { "PASS" } else { "FAIL" };
        let mut line = format!("Harness (a throwaway test outside the repo, run by the app): {verdict}");
        if h.cases.is_empty() {
            line.push_str(" (by exit code only)");
        } else {
            let ok = h.cases.iter().filter(|c| c.ok).count();
            line.push_str(&format!(" {ok}/{}", h.cases.len()));
        }
        let failed: Vec<&str> = h
            .cases
            .iter()
            .filter(|c| !c.ok)
            .map(|c| c.name.as_str())
            .collect();
        if !failed.is_empty() {
            line.push_str(&format!("; failed: {}", names(failed)));
        }
        lines.push(line);
    }
    if let Some(b) = s
        .browser
        .as_ref()
        .filter(|b| s.workflow.uses(StepKind::BrowserTest) && b.runs > 0)
    {
        let tested = b.cases.iter().filter(|c| !c.untested).count();
        let ok = b.cases.iter().filter(|c| c.ok && !c.untested).count();
        let mut line = format!("Browser test (an AI used the running app in Chrome): OK {ok}/{tested}");
        let ng: Vec<&str> = b
            .cases
            .iter()
            .filter(|c| !c.ok && !c.untested)
            .map(|c| c.name.as_str())
            .collect();
        let untested: Vec<&str> = b
            .cases
            .iter()
            .filter(|c| c.untested)
            .map(|c| c.name.as_str())
            .collect();
        if !ng.is_empty() {
            line.push_str(&format!("; NG: {}", names(ng)));
        }
        if !untested.is_empty() {
            line.push_str(&format!("; not tested: {}", names(untested)));
        }
        lines.push(line);
    }
    lines
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
        stdin: None,
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

fn checkpoint_harness(inner: &Inner, wp_id: u64, state: &HarnessState) {
    if let Some(entry) = inner.lock_tasks().get_mut(&wp_id) {
        entry.saved.harness = Some(state.clone());
        inner.persist(wp_id, entry);
    }
}

fn harness_repo_unchanged(repo: &Path, before: &GitSnap) -> Result<(), FlowError> {
    let after = git_snap(repo).map_err(|e| flow_error("repo.git", e))?;
    match git_surprise(StepKind::Harness, before, &after) {
        Some(reason) => Err(flow_error(
            "git.unexpected",
            format!("Harness dừng vì {reason}. Kiểm tra repo trước khi thử lại."),
        )),
        None => Ok(()),
    }
}

fn other_agent(builder: AgentKind) -> AgentKind {
    if builder == AgentKind::Codex {
        AgentKind::Claude
    } else {
        AgentKind::Codex
    }
}

fn review_harness(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    repo: &Path,
    state: &HarnessState,
) -> Result<Option<f64>, FlowError> {
    let builder = state.builder.unwrap_or(claim.agent);
    let agent = other_agent(builder);
    let review = Claim {
        step: StepKind::Harness,
        agent,
        stop: claim.stop.clone(),
        choice: ModelChoice::default(),
        mode: None,
        // the app reads its findings; the harness build already reported to the app
        nested: true,
    };
    let brief = ctx.read(BRIEF_FILE).unwrap_or_default();
    let plan = ctx.read(PLAN_FILE).unwrap_or_default();
    let prompt = prompts::review_harness(&brief, &plan, &state.dir, &state.command());
    let before = git_snap(repo).map_err(|e| flow_error("repo.git", e))?;
    if let Some(run) = inner
        .lock_tasks()
        .get_mut(&ctx.row.wp_id)
        .and_then(|e| e.run.as_mut())
    {
        run.agent = agent;
        run.command = Some("Review harness".into());
        run.model = None;
        run.effort = None;
    }
    inner.on_tool(
        ctx.row.wp_id,
        StepKind::Harness,
        agent_name(agent),
        "Review harness",
    );
    let (raw, cost) = if agent == AgentKind::Codex {
        (
            codex_read_review(inner, ctx, &review, "harness-review", prompt)?,
            None,
        )
    } else {
        let (budget, timeout, _) = limits(StepKind::Harness, &ctx.config);
        let job = Job {
            agent: Provider::Claude,
            args: claude::analyze_args(budget, std::slice::from_ref(&state.dir)),
            cwd: repo.to_path_buf(),
            stdin: Some(prompt),
            timeout,
            mcp_token: None,
        };
        let r = inner.run_claude(ctx, StepKind::Harness, &job, &review)?;
        (
            text::findings_from_text(&r.text).ok_or_else(unreadable_review)?,
            Some(r.cost_usd),
        )
    };
    harness_repo_unchanged(repo, &before)?;
    inner.merge(ctx, StepKind::Harness, 0, HARNESS_REVIEW, &raw)?;
    Ok(cost)
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
        .filter(|h| h.reviewed && !h.run.is_empty() && Path::new(&h.dir).is_dir());
    let Some(state) = state else { return Ok(None) };
    if inner
        .findings(wp_id)
        .iter()
        .any(|f| f.source == HARNESS_REVIEW && !settled(&f.state))
    {
        return Ok(None);
    }
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
    let (saved, checks) = inner
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
            (s.harness.clone(), checks)
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
        let (event, _) = review_event(StepKind::Harness, &inner.findings(wp_id), &raw);
        Done {
            event,
            note: format!("{tally}{negative}{extra}"),
            cost,
        }
    };
    let (mut state, built_cost) = if let Some(state) = saved
        .as_ref()
        .filter(|h| !h.run.is_empty() && Path::new(&h.dir).is_dir())
    {
        (state.clone(), None)
    } else {
        build_harness(inner, ctx, claim, repo, &checks, saved.map_or(0, |s| s.runs))?
    };
    let was_reviewed = state.reviewed;
    let reviewed_cost = if state.reviewed {
        None
    } else {
        let cost = review_harness(inner, ctx, claim, repo, &state)?;
        state.reviewed = true;
        checkpoint_harness(inner, wp_id, &state);
        cost
    };
    let cost = match (built_cost, reviewed_cost) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or_default() + b.unwrap_or_default()),
    };
    let open = inner
        .findings(wp_id)
        .iter()
        .filter(|f| f.source == HARNESS_REVIEW && !settled(&f.state))
        .count();
    if open > 0 {
        return Ok(Done {
            event: WorkflowEvent::ReviewFinished { open },
            note: format!("Review harness báo {open} lỗi cần xử lý"),
            cost,
        });
    }
    let ran = harness_command(inner, ctx, claim, repo, &state)?;
    let note = if was_reviewed {
        " (app chạy lại, không tốn token)"
    } else {
        " (review harness xong)"
    };
    Ok(finish(state, ran, cost, note.into()))
}

fn build_harness(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    repo: &Path,
    checks: &[String],
    runs: u32,
) -> Result<(HarnessState, Option<f64>), FlowError> {
    let (budget, timeout, _) = limits(StepKind::Harness, &ctx.config);
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
        &ctx.read(BRIEF_FILE).unwrap_or_default(),
        &plan,
        checks,
        &path(&scratch),
        &path(&library),
        shell_of(claim, &allowed),
    );
    let job = code_job(
        claim,
        None,
        &[path(&scratch)],
        budget,
        &repo_config.allow_bash,
        repo,
        timeout,
        prompt,
    );
    let before = git_snap(repo).map_err(|e| flow_error("repo.git", e))?;
    let r = inner.run_ai(ctx, StepKind::Harness, &job, claim)?;
    harness_repo_unchanged(repo, &before)?;
    let answer = harness::answer(&r.text).ok_or_else(|| {
        flow_error(
            "harness.parse",
            "AI không trả về kết quả harness (khối ```json có `result` và `run`). Xem tab Chat rồi bấm Thử lại.",
        )
    })?;
    let state = HarnessState {
        run: answer.run.clone(),
        dir: path(&scratch),
        builder: Some(claim.agent),
        summary: answer.summary.clone(),
        negative: answer.negative.as_ref().map(|(ok, _)| *ok),
        negative_detail: answer.negative.clone().map(|(_, d)| d).unwrap_or_default(),
        files: answer.files.clone(),
        library_root: Some(path(&library)),
        pending: answer.library.clone(),
        runs,
        ..HarnessState::default()
    };
    checkpoint_harness(inner, ctx.row.wp_id, &state);
    Ok((state, r.cost))
}

/// The findings a Sửa lỗi run took (marked `fixing` before the AI starts). When the step does not
/// end well (stopped, failed, verdict missing, Kiểm tra still red) they go back as they were, so
/// Thử lại / Chạy tiếp fixes them again instead of finding nothing left to fix and passing.
struct TakenFindings<'a> {
    store: &'a Mutex<Store>,
    rows: Vec<FindingRow>,
}

impl TakenFindings<'_> {
    /// The step ended well: leave the findings where the run put them.
    fn keep(mut self) {
        self.rows.clear();
    }
}

impl Drop for TakenFindings<'_> {
    fn drop(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        if let Ok(store) = self.store.lock() {
            for f in &self.rows {
                let _ = store.upsert_finding(f);
            }
        }
    }
}

fn settle_harness_review_fixes(
    inner: &Inner,
    wp_id: u64,
    items: &[(&FindingRow, bool)],
    reply: &str,
) -> Result<(), FlowError> {
    if !items.iter().any(|(f, _)| f.source == HARNESS_REVIEW) {
        return Ok(());
    }
    let fixed: Vec<usize> = text::fenced_blocks(reply)
        .into_iter()
        .rev()
        .find_map(|block| {
            let value = serde_json::from_str::<serde_json::Value>(&block.body).ok()?;
            serde_json::from_value(value.get("fixed")?.clone()).ok()
        })
        .ok_or_else(|| {
            flow_error(
                "harness.fixReport",
                "AI chưa báo các lỗi review harness đã sửa. Xem Chat rồi bấm Thử lại.",
            )
        })?;
    let store = inner.lock_store().map_err(|e| flow_error(&e.code, e.message))?;
    for (i, (f, _)) in items
        .iter()
        .enumerate()
        .filter(|(_, (f, _))| f.source == HARNESS_REVIEW)
    {
        let state = if fixed.contains(&(i + 1)) { "fixed" } else { "open" };
        store
            .upsert_finding(&FindingRow {
                state: state.into(),
                chosen: false,
                ..(*f).clone()
            })
            .map_err(|e| {
                flow_error(
                    "storage",
                    format!("Không lưu được kết quả sửa Harness {wp_id}: {e}"),
                )
            })?;
    }
    Ok(())
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
    let (scenario, runs, note) = inner
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
            (scenario, s.browser.as_ref().map_or(0, |b| b.runs), s.note.clone())
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
    let configured = inner
        .with_saved(wp_id, |s| dev_server_of(ctx, s))
        .flatten()
        .map(|(d, _)| d);
    let how_to = configured
        .as_ref()
        .map(|d| d.how_to.trim().to_owned())
        .filter(|h| !h.is_empty());
    let server = configured.filter(|d| !d.url.trim().is_empty());
    // the user answered what the AI asked: go on in that session, with the window left open
    let answered = inner
        .with_saved(wp_id, |s| {
            s.ask
                .take()
                .filter(|a| a.answer.is_some() && !a.session.is_empty())
        })
        .flatten();
    // "Chạy tiếp" after Dừng: the stopped session, the window left as it was
    let go_on = inner.take_go_on(wp_id, StepKind::BrowserTest, AgentKind::Claude);
    let mut session = inner
        .browsers
        .lock()
        .ok()
        .and_then(|mut b| b.remove(&wp_id))
        .unwrap_or_default();
    let had_window = session.chrome.is_some();
    let mut restarted = false;
    if let Some((chrome, _)) = session.chrome.as_mut() {
        if chrome.exited().is_some() {
            session.chrome = None;
            restarted = true;
        }
    }
    if session.server.as_mut().is_some_and(|p| p.exited().is_some()) {
        // one of them died: stop the rest too, then all start again below
        if let Some(p) = session.server.take() {
            p.stop();
        }
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
    // screenshots go to the task folder: the MCP server writes only where it is told
    let mcp_json = claude::browser_mcp(session.chrome.as_ref().map(|(_, port)| *port), &path(&shots));
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
    let prompt = match (&answered, &go_on) {
        (Some(a), _) => prompts::browser_answer(a.answer.as_deref().unwrap_or_default(), restarted),
        (None, Some(g)) => prompts::go_on(&g.note, Some(had_window && !restarted)),
        (None, None) => prompts::browser_test(&prompts::BrowserTest {
            plan: &plan,
            brief,
            scenario: &scenario,
            shots: &path(&shots),
            app,
            memory: memory.as_deref().map(path).as_deref(),
            how_to: how_to.as_deref(),
            note: note.as_deref(),
        }),
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
    let resume = answered
        .as_ref()
        .map(|a| a.session.clone())
        .or_else(|| go_on.as_ref().map(|g| g.session.clone()));
    if let Some(id) = resume {
        args.splice(1..1, ["--resume".to_owned(), id]);
    }
    let job = Job {
        agent: Provider::Claude,
        args,
        cwd: repo.to_path_buf(),
        stdin: Some(prompt),
        timeout,
        mcp_token: None,
    };
    let ran = inner.run_claude(ctx, StepKind::BrowserTest, &job, claim);
    // Dừng: keep the window and the app as they are, for "Chạy tiếp" (closed when the task
    // goes another way, goes back to the Inbox, or the app quits)
    let ran = match ran {
        Err(e) if e.code == "step.stopped" && !inner.quitting.load(Ordering::Relaxed) => {
            let open = session.chrome.is_some() || session.server.is_some();
            if let Ok(mut b) = inner.browsers.lock() {
                b.insert(wp_id, session);
            }
            if open {
                inner.log(
                    wp_id,
                    Some(StepKind::BrowserTest),
                    "info",
                    "Cửa sổ Chrome test (và dev server) vẫn mở: Chạy tiếp làm tiếp đúng trang đó",
                    false,
                );
            }
            return Err(e);
        }
        other => other,
    };
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
    // what the user wrote with Làm tiếp was for this run
    inner.with_saved(wp_id, |s| {
        if note.is_some() {
            s.note = None;
        }
        s.browser = Some(BrowserState {
            passed: answer.passed(),
            summary: answer.summary.clone(),
            url: answer.url.clone(),
            cases: browsertest::shown(&answer),
            at: now(),
            runs: runs + 1,
            changes: browsertest::shown_changes(&answer),
        });
    });
    for change in &answer.changes {
        inner.log(
            wp_id,
            Some(StepKind::BrowserTest),
            "info",
            &format!("AI đã thay đổi dữ liệu thật: {}", short(change)),
            false,
        );
    }
    let untested = answer.untested();
    for c in &untested {
        let why = if c.detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", short(&c.detail))
        };
        inner.log(
            wp_id,
            Some(StepKind::BrowserTest),
            "info",
            &format!("Chưa test được: {}{why}", c.name),
            false,
        );
    }
    if untested.len() == answer.cases.len() {
        return Err(flow_error(
            "browser.untested",
            "AI không test được ca nào (lý do ở tab Tiến trình). Sửa Cách vào chỗ test ở tab Plan nếu cần, rồi bấm Thử lại.",
        ));
    }
    let raw = browsertest::findings(&answer);
    let skipped = browsertest::untested_fingerprints(&answer);
    let findings = inner.merge_untested(ctx, StepKind::BrowserTest, 0, "browserTest", &raw, &skipped)?;
    let (event, _) = review_event(StepKind::BrowserTest, &findings, &raw);
    let pictures = answer.cases.iter().filter(|c| c.screenshot.is_some()).count();
    let mut note = browsertest::tally(&answer);
    if pictures > 0 {
        note.push_str(&format!(", {pictures} ảnh"));
    }
    if !answer.changes.is_empty() {
        note.push_str(&format!(", đổi {} dữ liệu thật", answer.changes.len()));
    }
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
    inner.on_tool(
        wp_id,
        StepKind::BrowserTest,
        "Dev server",
        &devserver::what_runs(server),
    );
    let log = shots.join("dev-server.log");
    match devserver::bring_up(&*starter, repo, server, &log, devserver::READY_WAIT, &claim.stop) {
        Ok(Up { started, lines }) => {
            for line in &lines {
                inner.log(wp_id, Some(StepKind::BrowserTest), "info", line, false);
            }
            Ok(started)
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
        mcp_token: None,
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
            let mut prompt = prompts::analyze(&brief, previous);
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
            if r.manual && answer.plan.is_empty() {
                return Ok(Done {
                    event: E::AnalysisDoneByUser,
                    note: "Bạn đánh dấu xong trong Workspace".into(),
                    cost: r.cost,
                });
            }
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
            let mut proposals: Vec<StepProposal> = answer
                .proposals
                .into_iter()
                .filter_map(|mut p| {
                    let step = match p.step {
                        "harness" => StepKind::Harness,
                        "browserTest" => StepKind::BrowserTest,
                        _ => return None,
                    };
                    if step == StepKind::Harness {
                        let has_checks = p
                            .checks
                            .lines()
                            .any(|line| !line.trim_start_matches(['-', ' ']).trim().is_empty());
                        if p.reason.trim().is_empty() || (p.recommend && !has_checks) {
                            p.recommend = false;
                            p.reason = NO_HARNESS_REASON.into();
                        }
                        if !p.recommend {
                            p.checks.clear();
                        }
                    }
                    Some(StepProposal {
                        step,
                        recommend: p.recommend,
                        reason: p.reason,
                        checks: p.checks,
                        dev_server: p.dev_server.map(|d| DevServer {
                            url: d.url,
                            how_to: String::new(),
                            commands: d
                                .commands
                                .into_iter()
                                .map(|c| DevCommand {
                                    command: c.command,
                                    dir: c.dir,
                                    wait: c.wait,
                                })
                                .collect(),
                        }),
                    })
                })
                .collect();
            if !proposals.iter().any(|p| p.step == StepKind::Harness) {
                proposals.push(StepProposal {
                    step: StepKind::Harness,
                    recommend: false,
                    reason: NO_HARNESS_REASON.into(),
                    checks: String::new(),
                    dev_server: None,
                });
            }
            let note = plan_note(&answer.plan, &proposals);
            inner.with_saved(wp_id, |s| {
                if let Some(h) = s.harness.as_mut() {
                    h.run.clear();
                }
                // the AI's advice turns a step on or off unless the user already chose
                for p in &proposals {
                    if !s.picked.contains(&p.step) && s.workflow.state_of(p.step) == StepState::Pending {
                        s.workflow.set_uses(p.step, p.recommend);
                    }
                }
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
                .or_else(|| {
                    inner
                        .sessions()
                        .map(|_| "Use the task brief and the decisions in this conversation.".into())
                })
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
            inner.with_saved(wp_id, |s| {
                s.note = None;
                s.coded_by_hand = false;
            });
            Ok(Done {
                event: E::StepSucceeded(StepKind::Implement),
                note: format!("{}{}", change_note(repo), denied_note(r.denials)),
                cost: r.cost,
            })
        }
        // Review chốt: the code is still what Tự review saw, so its result stands
        StepKind::FinalReview if unchanged_since_self_review(inner, wp_id, repo, &base) => Ok(Done {
            event: E::StepSkipped(StepKind::FinalReview),
            note: FINAL_REVIEW_KEPT.into(),
            cost: None,
        }),
        StepKind::ReviewLocal | StepKind::FinalReview if codex => {
            // Codex cannot run the repo's /review-local: the app's review prompt, read only
            let plan = ctx.read(PLAN_FILE).unwrap_or_default();
            let raw = codex_review(
                inner,
                ctx,
                claim,
                &format!("codex-{}", step_key(step)),
                &base,
                &brief,
                &plan,
            )?;
            let findings = inner.merge(ctx, step, 0, "codex", &raw)?;
            let (event, note) = review_event(step, &findings, &raw);
            mark_reviewed(inner, wp_id, step, repo, &base);
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
            let plan = ctx.read(PLAN_FILE).unwrap_or_default();
            // Tự review goes on in the code session, like /review-local typed right after coding:
            // it knows the task and why the code is so. Review chốt, code written by hand and
            // code another AI wrote get a fresh session.
            let by_hand = inner.with_saved(wp_id, |s| s.coded_by_hand).unwrap_or_default();
            let resume = session
                .as_deref()
                .filter(|_| step == StepKind::ReviewLocal && !by_hand);
            let path = |p: &Path| p.to_string_lossy().into_owned();
            let (dir, brief_file, plan_file) = (
                path(&ctx.task_dir),
                path(&ctx.task_dir.join(BRIEF_FILE)),
                path(&ctx.task_dir.join(PLAN_FILE)),
            );
            let task = claude::TaskFiles {
                dir: &dir,
                brief: &brief_file,
                plan: &plan_file,
            };
            let (args, stdin, source) = if has_command {
                (
                    claude::review_local_args(&base, budget, resume, Some(task)),
                    None,
                    "review-local",
                )
            } else {
                (
                    claude::review_args(budget, resume),
                    Some(prompts::review(&base, &brief, &plan)),
                    "claude",
                )
            };
            let job = job(Provider::Claude, cl(args), stdin);
            let shared = inner.sessions().filter(|_| step == StepKind::ReviewLocal);
            let (r, manual) = if let Some(sessions) = shared {
                let reply = inner.run_main_session(&sessions, ctx, step, &job, claim)?;
                (reply.result, reply.manual)
            } else {
                (inner.run_claude(ctx, step, &job, claim)?, false)
            };
            if resume.is_some() {
                set_session(&r.session_id);
            }
            let raw = text::findings_from_text(&r.text)
                .or_else(|| manual.then(Vec::new))
                .ok_or_else(unreadable_review)?;
            save_review_notes(ctx, step, &r.text);
            let findings = inner.merge(ctx, step, 0, source, &raw)?;
            let (event, note) = review_event(step, &findings, &raw);
            mark_reviewed(inner, wp_id, step, repo, &base);
            Ok(Done {
                event,
                note: match resume {
                    Some(_) => format!("{note} (trong phiên Viết code)"),
                    None => note,
                },
                cost: Some(r.cost_usd),
            })
        }
        StepKind::Fix | StepKind::FixCross => {
            let findings = inner.findings(wp_id);
            let picks = inner
                .with_saved(wp_id, |s| s.harness_fix.clone())
                .unwrap_or_default();
            let items: Vec<(&FindingRow, bool)> = findings
                .iter()
                .filter(|f| to_fix(f))
                .map(|f| {
                    (
                        f,
                        f.source == HARNESS_REVIEW
                            || (f.source == "harness" && picks.contains(&f.fingerprint)),
                    )
                })
                .collect();
            if items.is_empty() {
                return Ok(Done {
                    event: E::StepSucceeded(step),
                    note: "không có lỗi nào được chọn để sửa".into(),
                    cost: None,
                });
            }
            if let Ok(store) = inner.store.lock() {
                for (f, _) in &items {
                    let _ = store.upsert_finding(&FindingRow {
                        state: "fixing".into(),
                        ..(*f).clone()
                    });
                }
            }
            let taken = TakenFindings {
                store: &inner.store,
                rows: items.iter().map(|(f, _)| (*f).clone()).collect(),
            };
            let list: Vec<FixItem> = items
                .iter()
                .map(|(f, harness)| FixItem {
                    severity: f.severity.clone(),
                    file: f.file.clone(),
                    line: f.line,
                    title: if *harness {
                        format!("[Sửa harness] {}", f.title)
                    } else {
                        f.title.clone()
                    },
                    detail: f.detail.clone().unwrap_or_default(),
                    suggestion: f.suggestion.clone().unwrap_or_default(),
                })
                .collect();
            let has_harness = items.iter().any(|(_, h)| *h);
            let has_code = items.iter().any(|(_, h)| !h);
            let scratch = if has_harness {
                let dir = scratch_dir(inner, ctx);
                if !dir.is_dir() {
                    return Err(flow_error(
                        "harness.scratch",
                        "Không thấy thư mục harness. Quay lại bước Harness để dựng lại.",
                    ));
                }
                Some(dir)
            } else {
                None
            };
            let dirs: Vec<String> = scratch.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            let mut allowed = ctx.repo_config().allow_bash;
            if has_harness {
                allowed.extend(claude::HARNESS_BASH.iter().map(|s| (*s).to_owned()));
            }
            let scratch_only = (!has_code).then(|| Claim {
                step,
                agent: claim.agent,
                stop: claim.stop.clone(),
                choice: claim.choice.clone(),
                mode: None,
                nested: false,
            });
            let claim = scratch_only.as_ref().unwrap_or(claim);
            let shell = shell_of(claim, &allowed);
            let cut = inner.cut_session(wp_id, step, provider_of(claim.agent));
            // no session of this AI: it did not write the code, so it gets the plan and the brief
            let plan = ctx.read(PLAN_FILE).unwrap_or_default();
            let context = session.is_none().then_some((plan.as_str(), brief.as_str()));
            let mut prompt = match (scratch.as_ref(), has_code, &cut) {
                (Some(dir), false, _) => prompts::fix_harness(
                    &list,
                    &brief,
                    &plan,
                    &repo.to_string_lossy(),
                    &dir.to_string_lossy(),
                ),
                (Some(dir), true, _) => {
                    let mut prompt = prompts::fix(&list, shell, Some((&plan, &brief)));
                    prompt.push_str(&format!(
                        "\nItems marked [Sửa harness] must be fixed only in `{}` using the brief and plan; \
                         never restore removed code to make a test pass. For the other items edit \
                         production code. End with a JSON block {{\"fixed\":[1,2]}} listing only the \
                         numbers of harness findings you actually fixed.",
                        dir.display()
                    ));
                    prompt
                }
                (None, _, Some(_)) => prompts::carry_on_cut(step_label(step), shell),
                (None, _, None) => prompts::fix(&list, shell, context),
            };
            if cut.is_none() && inner.was_cut(wp_id, step) {
                prompt.push_str(prompts::CUT_BEFORE);
            }
            let resume = cut.as_deref().or(session.as_deref().filter(|_| has_code));
            let cwd = scratch.as_deref().filter(|_| !has_code).unwrap_or(repo);
            let mut code = code_job(claim, resume, &dirs, budget, &allowed, cwd, timeout, prompt);
            if !has_code && claim.agent == AgentKind::Codex {
                code.args.insert(1, "--skip-git-repo-check".into());
            }
            let before = (!has_code)
                .then(|| git_snap(repo))
                .transpose()
                .map_err(|e| flow_error("repo.git", e))?;
            let r = inner.run_ai(ctx, step, &code, claim)?;
            if let Some(before) = before {
                harness_repo_unchanged(repo, &before)?;
            } else {
                set_session(&r.session);
            }
            settle_harness_review_fixes(inner, wp_id, &items, &r.text)?;
            let fixes = inner
                .with_saved(wp_id, |s| {
                    s.fixes += 1;
                    s.fixes
                })
                .unwrap_or(fixes + 1);
            // Kiểm tra again on what the fix changed; it belongs to this step (no own row)
            let again = if has_code {
                run_checks(inner, ctx, claim, repo, &base, step)?
            } else {
                ChecksDone {
                    ran: false,
                    note: String::new(),
                    cost: 0.0,
                }
            };
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
            taken.keep();
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
            // a fresh session that checks the change against the task as well as the code
            let plan = ctx.read(PLAN_FILE).unwrap_or_default();
            let (raw, source, cost) = if codex {
                let raw = codex_review(
                    inner,
                    ctx,
                    claim,
                    &format!("codex-r{round}"),
                    &base,
                    &brief,
                    &plan,
                )?;
                (raw, "codex", None)
            } else {
                let stdin = Some(prompts::review(&base, &brief, &plan));
                let r = inner.run_claude(
                    ctx,
                    step,
                    &job(Provider::Claude, cl(claude::review_args(budget, None)), stdin),
                    claim,
                )?;
                let raw = text::findings_from_text(&r.text).ok_or_else(unreadable_review)?;
                save_review_notes(ctx, step, &r.text);
                (raw, "claude", Some(r.cost_usd))
            };
            let findings = inner.merge(ctx, step, round, source, &raw)?;
            let (event, note) = review_event(step, &findings, &raw);
            Ok(Done {
                event,
                note: format!("vòng {round}: {note}"),
                cost,
            })
        }
        StepKind::PushSummary => {
            // "Không sửa" at a findings gate is a decision; a finding picked for Sửa and never
            // fixed is not
            let picked = inner.findings(wp_id).iter().filter(|f| to_fix(f)).count();
            if inner.require_mcp && picked > 0 {
                return Err(flow_error(
                    "mcp.openFindings",
                    format!(
                        "Còn {picked} lỗi bạn đã chọn Sửa nhưng chưa sửa. Mở tab Lỗi review: chạy Sửa lỗi, hoặc Bỏ chọn / Chấp nhận / Để sau."
                    ),
                ));
            }
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
            // the app's prompt knows the task and what was really tested on it
            let app_prompt = || {
                let plan = ctx.read(PLAN_FILE).unwrap_or_default();
                let checked = inner.with_saved(wp_id, checked_lines).unwrap_or_default();
                prompts::push_summary(&base, issue, &brief, &plan, &checked)
            };
            let push = if codex {
                // Codex cannot run the repo's /push-summary: the app's prompt
                let stdin = Some(app_prompt());
                job(Provider::Codex, codex_cut(cx(codex::push_args())), stdin)
            } else if has_command {
                job(
                    Provider::Claude,
                    cl(claude::push_summary_args(issue, budget)),
                    None,
                )
            } else {
                let stdin = Some(app_prompt());
                job(
                    Provider::Claude,
                    cl(claude::push_summary_default_args(budget)),
                    stdin,
                )
            };
            let r = inner.run_ai(ctx, step, &push, claim)?;
            if r.text.trim().is_empty() && !r.manual {
                return Err(flow_error(
                    "push.empty",
                    format!(
                        "{} không trả về commit message. Xem Log rồi bấm Thử lại.",
                        agent_name(claim.agent)
                    ),
                ));
            }
            if !r.text.trim().is_empty() {
                write_task_file(ctx, PUSH_FILE, &r.text)?;
            }
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
    plan: &str,
) -> Result<Vec<RawFinding>, FlowError> {
    codex_read_review(inner, ctx, claim, name, prompts::cross_review(base, brief, plan))
}

fn codex_read_review(
    inner: &Inner,
    ctx: &TaskCtx,
    claim: &Claim,
    name: &str,
    prompt: String,
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
        stdin: Some(prompt),
        timeout,
        mcp_token: None,
    };
    let (answer, _) = inner.run_codex(ctx, step, &job, claim)?;
    let text = std::fs::read_to_string(&out).unwrap_or(answer);
    let raw = serde_json::from_str::<serde_json::Value>(text.trim())
        .ok()
        .and_then(|v| text::findings_from_json(&v))
        .or_else(|| text::findings_from_text(&text))
        .ok_or_else(unreadable_review)?;
    if step.is_review() {
        save_review_notes(ctx, step, &text);
    }
    Ok(raw)
}

/// Runs the claimed step, then the next ones while auto-advance is on.
fn worker(inner: &Arc<Inner>, ctx: &TaskCtx, first: Claim) {
    let mut claim = first;
    let mut ctx = ctx.clone();
    loop {
        let step = claim.step;
        let result = execute(inner, &ctx, &claim);
        if let Some(config) = inner
            .config_path
            .lock()
            .ok()
            .and_then(|p| p.as_ref().and_then(|p| Config::load(p).ok()))
        {
            ctx.config = config;
        }
        match inner.finish(&ctx, step, result) {
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
        } else if has("--no-session-persistence") || flag(job, "--tools").as_deref() == Some("Bash") {
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
        let flow = Flow::new_without_mcp(s.store.clone(), fake.clone(), None);
        // never a real Chrome or dev server in tests (it made the browser test slow and flaky)
        flow.set_app_starter(Arc::new(FakeStarter::default()));
        (flow, fake)
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

    #[test]
    fn workspace_steps_need_something_to_work_on_and_start_over_goes_back_to_analysis() {
        let Some(s) = setup() else { return };
        let (flow, _fake) = flow_with(&s, false, happy(CLEAN));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        flow.act(ctx.clone(), FlowAction::SetWorkspaceFlow { on: true })
            .unwrap();
        let why = |step: StepKind| {
            flow.workspace(&ctx)
                .steps
                .into_iter()
                .find(|s| s.step == step)
                .and_then(|s| s.why_not)
        };
        // nothing done yet: analysis and coding can run, a fix or a review cannot
        assert_eq!(why(StepKind::Analyze), None);
        assert_eq!(why(StepKind::Implement), None);
        assert!(why(StepKind::Fix).is_some_and(|w| w.contains("lỗi")));
        assert!(why(StepKind::ReviewLocal).is_some_and(|w| w.contains("code")));
        let refused = flow
            .act(ctx.clone(), FlowAction::RunWorkspaceStep { step: StepKind::Fix })
            .unwrap_err();
        assert_eq!(refused.code, "step.notReady");
        assert_eq!(flow.workspace(&ctx).running, None);
        // a change on the branch is code to review; a fix still needs a picked finding
        std::fs::write(
            s.repo.join("a.ts"),
            "export const a = 2;
",
        )
        .unwrap();
        let refused = flow
            .act(
                ctx.clone(),
                FlowAction::RunWorkspaceStep {
                    step: StepKind::FixCross,
                },
            )
            .unwrap_err();
        assert_eq!(refused.code, "step.notReady");
        assert_eq!(why(StepKind::ReviewLocal), None);
        assert!(why(StepKind::FixCross).is_some());

        // Làm lại từ đầu: back at Phân tích, the Workspace flow stays on
        flow.act(ctx.clone(), FlowAction::SetAuto { on: true }).unwrap();
        flow.act(ctx.clone(), FlowAction::StartOver).unwrap();
        let view = flow.workspace(&ctx);
        assert!(view.enabled && view.auto);
        let analyze = view.steps.iter().find(|s| s.step == StepKind::Analyze).unwrap();
        assert_eq!(analyze.state, StepState::Running);
        assert!(
            view.steps
                .iter()
                .filter(|s| s.step != StepKind::Analyze)
                .all(|s| s.state != StepState::Done)
        );
    }

    mod followup {
        include!("flow_followup_tests.rs");
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
            StepKind::FinalReview,
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

    /// The user pressed Dừng (the fake ends the run as stopped).
    fn stopped() -> RunOutcome {
        RunOutcome {
            stopped: true,
            ..RunOutcome::default()
        }
    }

    /// Dừng, then "Chạy tiếp": the stopped run's own session takes the user's words; "Chạy lại
    /// từ đầu" starts a new one instead.
    #[test]
    fn after_stop_the_user_goes_on_in_the_same_session_with_a_note() {
        let Some(s) = setup() else { return };
        let analyses = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| {
            if kind(job) == "analyze" && analyses.fetch_add(1, Ordering::SeqCst) == 0 {
                out(&init_line());
                return stopped();
            }
            (happy(CLEAN))(job, stop, out)
        });
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        let err = d.error.clone().unwrap();
        assert_eq!(
            (err.code.as_str(), err.message.as_str()),
            ("step.stopped", STOPPED_GO_ON)
        );
        assert!(d.can_go_on);

        let d = run(
            &flow,
            &ctx,
            FlowAction::GoOn {
                note: " Đọc thêm a.ts ".into(),
            },
        );
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::PlanReview),
            "{:?}",
            d.error
        );
        assert!(!d.can_go_on);
        let jobs = fake.jobs.lock().unwrap().clone();
        let again = &jobs[1];
        assert!(
            again.args.windows(2).any(|w| w == ["--resume", "s-1"]),
            "{:?}",
            again.args
        );
        let prompt = again.stdin.as_deref().unwrap();
        assert!(
            prompt.contains("<user_note>\nĐọc thêm a.ts\n</user_note>") && prompt.contains("same format")
        );
        assert!(
            d.log
                .iter()
                .any(|l| l.text == "Chạy tiếp Phân tích: Đọc thêm a.ts")
        );
        // only right after a Dừng
        assert_eq!(
            flow.act(ctx.clone(), FlowAction::GoOn { note: String::new() })
                .unwrap_err()
                .code,
            "flow.notNow"
        );
        // Codex can go on only where it writes code; the push summary keeps no session
        assert!(can_go_on(StepKind::Fix, AgentKind::Codex));
        assert!(!can_go_on(StepKind::CrossReview, AgentKind::Codex));
        assert!(!can_go_on(StepKind::PushSummary, AgentKind::Claude));
        assert!(can_go_on(StepKind::FinalReview, AgentKind::Claude));
    }

    /// Dừng in Test trình duyệt keeps its Chrome window open; "Chạy tiếp" goes on there.
    #[test]
    fn a_stopped_browser_test_keeps_its_window_and_goes_on_there() {
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
                    return stopped();
                }
                out(&result_line(
                    "```json\n{\"result\":\"ok\",\"summary\":\"ổn\",\"cases\":[{\"name\":\"1. mở /files\",\"ok\":true}]}\n```",
                ));
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
            if d.error.is_some() {
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
        assert_eq!(
            (d.current, d.error.as_ref().map(|e| e.code.as_str())),
            (Some(StepKind::BrowserTest), Some("step.stopped"))
        );
        assert!(d.can_go_on);
        // the window stays open for "Chạy tiếp"
        assert_eq!(starter.chrome_stops.load(Ordering::SeqCst), 0);
        assert!(
            d.log
                .iter()
                .any(|l| l.text.starts_with("Cửa sổ Chrome test (và dev server) vẫn mở"))
        );

        let d = run(
            &flow,
            &ctx,
            FlowAction::GoOn {
                note: "Mở thư mục test 4348".into(),
            },
        );
        assert_eq!(d.current, Some(StepKind::FinalReview), "{:?}", d.error);
        // the same window, the same session, then everything closed
        assert_eq!(starter.browsers.lock().unwrap().len(), 1);
        assert_eq!(starter.chrome_stops.load(Ordering::SeqCst), 1);
        let jobs = fake.jobs.lock().unwrap().clone();
        let runs: Vec<&Job> = jobs.iter().filter(|j| kind(j) == "browser").collect();
        assert_eq!(&runs[1].args[1..3], ["--resume", "s-1"]);
        let prompt = runs[1].stdin.as_deref().unwrap();
        assert!(
            prompt.contains("<user_note>\nMở thư mục test 4348\n</user_note>")
                && prompt.contains("still open"),
            "{prompt}"
        );
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
        // code written by hand: no code session to go on in, so Tự review starts fresh (the
        // session saved is the analysis's) with the brief and the plan
        let jobs = fake.jobs.lock().unwrap().clone();
        let review = jobs.last().unwrap();
        assert!(flag(review, "--resume").is_none(), "{review:?}");
        assert!(review.stdin.as_deref().unwrap().contains("<plan>"));
        assert!(
            !d.runs
                .last()
                .unwrap()
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("phiên Viết code")
        );
    }

    /// The mark Review chốt compares: any change to the code moves it, files git ignores do not.
    #[test]
    fn the_change_mark_follows_the_code() {
        let Some(s) = setup() else { return };
        let mark = || change_mark(&s.repo, "main").unwrap();
        let clean = mark();
        assert_eq!(mark(), clean);
        std::fs::write(s.repo.join("a.ts"), "export const a = 2;\n").unwrap();
        let edited = mark();
        assert_ne!(edited, clean);
        std::fs::write(s.repo.join("b.ts"), "export const b = 1;\n").unwrap();
        let added = mark();
        assert_ne!(added, edited);
        std::fs::write(s.repo.join("b.ts"), "export const b = 2;\n").unwrap();
        assert_ne!(mark(), added);
        std::fs::write(s.repo.join("b.ts"), "export const b = 1;\n").unwrap();
        assert_eq!(mark(), added);
        // what git ignores is not the task's code
        std::fs::write(s.repo.join(".gitignore"), "dist/\n").unwrap();
        let ignored = mark();
        std::fs::create_dir_all(s.repo.join("dist")).unwrap();
        std::fs::write(s.repo.join("dist").join("out.js"), "x").unwrap();
        assert_eq!(mark(), ignored);
        // no base to compare with: nothing is passed on it
        assert!(change_mark(&s.repo, "no-such-branch").is_none());
    }

    /// Tự review's high is fixed, so the code moved after it and Review chốt runs. Its answer has
    /// no findings block: the step fails, and Tiến trình shows a failed run although the process
    /// ended well.
    /// "Trao đổi với AI" answers on a thread of its own: wait for the answer.
    fn talked(flow: &Flow, ctx: &TaskCtx) -> TaskDetail {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let d = flow.detail(ctx);
            if !d.talk_busy {
                return d;
            }
            assert!(Instant::now() < deadline, "the AI never answered");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    const TALK_ANSWER: &str = "Lỗi vì thiếu quyền đọc thư mục cấu hình. Đề xuất: đọc file mẫu thay thế.";

    fn is_talk(job: &Job) -> bool {
        job.stdin
            .as_deref()
            .is_some_and(|p| p.contains("stopped with this error") || p.contains("same conversation"))
    }

    #[test]
    fn a_failed_step_is_talked_over_read_only_then_goes_on_in_its_own_session() {
        let Some(s) = setup() else { return };
        let analyses = AtomicUsize::new(0);
        let inner = happy(CLEAN);
        let (flow, fake) = flow_with(
            &s,
            false,
            move |job: &Job, stop: &AtomicBool, out: &mut dyn FnMut(&str)| {
                if is_talk(job) {
                    out(&result_line(TALK_ANSWER));
                    return ok();
                }
                if kind(job) == "analyze" && analyses.fetch_add(1, Ordering::SeqCst) == 0 {
                    out(&result_line("dừng giữa chừng"));
                    return RunOutcome {
                        code: Some(1),
                        ..RunOutcome::default()
                    };
                }
                inner(job, stop, out)
            },
        );
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::Error),
            "{:?}",
            d.error
        );
        assert!(!d.can_go_on, "a plain failure keeps no session to go on in");
        let empty = flow.act(ctx.clone(), FlowAction::Talk { text: " ".into() });
        assert_eq!(empty.unwrap_err().code, "talk.empty");

        flow.act(
            ctx.clone(),
            FlowAction::Talk {
                text: "Vì sao lỗi?".into(),
            },
        )
        .unwrap();
        let d = talked(&flow, &ctx);
        assert_eq!(d.talk.len(), 2);
        assert!(d.talk[0].from_user && !d.talk[1].from_user);
        assert!(d.talk[1].text.contains("thiếu quyền") && d.talk[1].cost_usd.is_some());
        assert!(d.can_go_on, "the talk went on in the step's own session");
        let talk = fake.jobs.lock().unwrap().last().unwrap().clone();
        assert!(
            talk.args.windows(2).any(|w| w == ["--resume", "s-1"]),
            "{:?}",
            talk.args
        );
        assert!(
            talk.args.windows(2).any(|w| w == ["--tools", "Read,Grep,Glob"]),
            "read only"
        );
        let prompt = talk.stdin.as_deref().unwrap();
        assert!(prompt.contains("<user_message>\nVì sao lỗi?") && prompt.contains("<error>"));
        assert!(
            !prompt.contains("<brief>"),
            "its own session already has the task"
        );

        // the next message stays in the same conversation
        flow.act(
            ctx.clone(),
            FlowAction::Talk {
                text: "Vậy làm tiếp đi".into(),
            },
        )
        .unwrap();
        assert_eq!(talked(&flow, &ctx).talk.len(), 4);
        let next = fake.jobs.lock().unwrap().last().unwrap().clone();
        assert!(next.stdin.as_deref().unwrap().contains("same conversation"));
        assert!(next.args.windows(2).any(|w| w == ["--resume", "s-1"]));

        // Chạy tiếp: the step goes on in that session; the talk is in there, so no copy of it
        let d = run(&flow, &ctx, FlowAction::GoOn { note: String::new() });
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::PlanReview),
            "{:?}",
            d.error
        );
        assert!(d.talk.is_empty());
        let went_on = fake.jobs.lock().unwrap().last().unwrap().clone();
        assert!(!is_talk(&went_on) && went_on.args.windows(2).any(|w| w == ["--resume", "s-1"]));
        assert!(!went_on.stdin.as_deref().unwrap().contains("earlier_conversation"));
    }

    #[test]
    fn talking_about_a_codex_step_starts_fresh_read_only_and_a_retry_starts_clean() {
        let Some(s) = setup() else { return };
        let mut config = Config::default();
        config.steps.analyze.agent = Some("codex".into());
        let analyses = AtomicUsize::new(0);
        let (flow, fake) = flow_with(
            &s,
            false,
            move |job: &Job, _stop: &AtomicBool, out: &mut dyn FnMut(&str)| {
                let say = |text: &str, out: &mut dyn FnMut(&str)| {
                    out(r#"{"type":"thread.started","thread_id":"t-1"}"#);
                    out(&serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":text}})
                        .to_string());
                };
                if is_talk(job) {
                    // a talk may only read: the app must notice a file it wrote
                    std::fs::write(job.cwd.join("talk-note.txt"), "x").unwrap();
                    say(TALK_ANSWER, out);
                    return ok();
                }
                if analyses.fetch_add(1, Ordering::SeqCst) == 0 {
                    return RunOutcome {
                        code: Some(1),
                        ..RunOutcome::default()
                    };
                }
                say("## Nguyên nhân\nx\n```json\n{\"questions\":[]}\n```", out);
                ok()
            },
        );
        let ctx = s.ctx(1, &config);
        flow.started(&ctx, None).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::Error),
            "{:?}",
            d.error
        );
        flow.act(
            ctx.clone(),
            FlowAction::Talk {
                text: "Có cần đổi cách làm không?".into(),
            },
        )
        .unwrap();
        let d = talked(&flow, &ctx);
        assert!(d.talk[1].text.contains("⚠ App thấy"), "{}", d.talk[1].text);
        assert!(
            !d.can_go_on,
            "Codex talks in a fresh session: nothing to go on in"
        );
        let talk = fake.jobs.lock().unwrap().last().unwrap().clone();
        assert_eq!(kind(&talk), "codex");
        assert!(talk.args.iter().any(|a| a == "read-only") && !talk.args.iter().any(|a| a == "resume"));
        assert!(
            talk.stdin.as_deref().unwrap().contains("<brief>"),
            "a fresh session gets the task"
        );
        std::fs::remove_file(s.repo.join("talk-note.txt")).unwrap();

        // Chạy lại từ đầu starts clean: the conversation stays out of the new run
        let d = run(&flow, &ctx, FlowAction::Retry);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::PlanReview),
            "{:?}",
            d.error
        );
        assert!(d.talk.is_empty());
        let retry = fake.jobs.lock().unwrap().last().unwrap().clone();
        let prompt = retry.stdin.as_deref().unwrap();
        assert!(!is_talk(&retry) && !prompt.contains("<earlier_conversation>"));
        assert!(!prompt.contains("Có cần đổi cách làm không?"));
        assert!(flow.inner.take_talk_carry(1, StepKind::Analyze).is_none());
    }

    #[test]
    fn a_checkout_of_another_repo_runs_and_a_moved_origin_starts_the_task_over() {
        let Some(s) = setup() else { return };
        git(
            &s.repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/secondary.git",
            ],
        );
        let mut config = Config::default();
        config.steps.verify_mcp = false;
        let ctx = s.ctx(1, &config);
        let binding =
            crate::task_repo::Binding::read(&Git::new(&s.repo), "https://github.com/example/main/issues/7")
                .unwrap();
        s.store
            .lock()
            .unwrap()
            .upsert_task_with_binding(
                &ctx.row,
                crate::task_repo::KEY,
                &serde_json::to_string(&binding).unwrap(),
            )
            .unwrap();
        let moves = AtomicUsize::new(0);
        let fake = Arc::new(Fake {
            script: Box::new(move |job: &Job, _stop: &AtomicBool, out: &mut dyn FnMut(&str)| {
                // the origin moves under the task during its first run
                if moves.fetch_add(1, Ordering::SeqCst) == 0 {
                    git(
                        &job.cwd,
                        &[
                            "remote",
                            "set-url",
                            "origin",
                            "https://github.com/example/moved.git",
                        ],
                    );
                }
                // a plan without the advice block is a plan
                out(&result_line(concat!(
                    "## Nguyên nhân\nbản phụ của repo chính; a.ts:1\n## Phương án\nA\n",
                    "## Đề xuất\nA\n## File sẽ sửa\na.ts\n## Kế hoạch test\nKiểm tra A\n## Rủi ro\na.ts:1\n",
                )));
                ok()
            }),
            jobs: Mutex::new(Vec::new()),
            missing: false,
        });
        let flow = Flow::new(s.store.clone(), fake.clone(), None);
        flow.set_app_starter(Arc::new(FakeStarter::default()));
        flow.started(&ctx, None).unwrap();
        // the checkout is not the issue's repo: the analysis runs, nothing tells it to judge that
        let d = run(&flow, &ctx, FlowAction::Run);
        let prompt = fake.jobs.lock().unwrap()[0].stdin.clone().unwrap();
        assert!(!prompt.contains("Repository check"));
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::PlanReview),
            "{:?}",
            d.error
        );
        // the origin moved during that run: the next step does not run with the old plan, the
        // task starts over from Phân tích bound to the checkout as it is now
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(fake.jobs.lock().unwrap().len(), 1, "Viết code did not run");
        assert_eq!(d.error, None);
        assert_eq!(d.current, Some(StepKind::Analyze));
        assert_eq!(d.status, FlowStatus::Ready);
        assert!(
            d.log
                .iter()
                .any(|l| l.text.contains("Repo của task đã đổi")
                    && l.text.contains("github.com/example/moved")),
            "{:?}",
            d.log
        );
        let bound: crate::task_repo::Binding = serde_json::from_str(
            &s.store
                .lock()
                .unwrap()
                .kv_get(1, crate::task_repo::KEY, "")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let json = serde_json::to_value(&bound).unwrap();
        assert_eq!(json["remote"], "github.com/example/moved");
        assert_eq!(bound.issue_repos, ["github.com/example/main"]);
    }

    #[test]
    fn a_failed_fix_gives_its_findings_back_and_thu_lai_fixes_them_again() {
        use opw_domain::finding::FindingState;
        let Some(s) = setup() else { return };
        let reviews = AtomicUsize::new(0);
        let fixes = AtomicUsize::new(0);
        let inner = happy(CLEAN);
        let is_fix =
            |job: &Job| kind(job) == "code" && job.stdin.as_deref().is_some_and(|p| p.contains("Null org"));
        let (flow, fake) = flow_with(
            &s,
            false,
            move |job: &Job, stop: &AtomicBool, out: &mut dyn FnMut(&str)| {
                if kind(job) == "review" {
                    let first = reviews.fetch_add(1, Ordering::SeqCst) == 0;
                    out(&result_line(if first { HIGH } else { CLEAN }));
                    return ok();
                }
                // the first fix dies before it reports anything
                if is_fix(job) && fixes.fetch_add(1, Ordering::SeqCst) == 0 {
                    return RunOutcome {
                        code: Some(1),
                        ..RunOutcome::default()
                    };
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
        pick_all(&flow, &ctx);
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
        assert!(d.error.is_some(), "{:?}", d.gate);
        // still picked and open, not left "fixing" as if the dead run had fixed it
        let f = flow
            .detail(&ctx)
            .findings
            .into_iter()
            .find(|f| f.title == "Null org")
            .unwrap();
        assert_eq!((f.state, f.chosen), (FindingState::Open, true));
        let d = run(&flow, &ctx, FlowAction::Retry);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::AfterFix),
            "{:?}",
            d.error
        );
        let jobs = fake.jobs.lock().unwrap().clone();
        assert_eq!(
            jobs.iter().filter(|j| is_fix(j)).count(),
            2,
            "Thử lại ran the fix again"
        );
        let f = flow
            .detail(&ctx)
            .findings
            .into_iter()
            .find(|f| f.title == "Null org")
            .unwrap();
        assert_eq!(f.state, FindingState::Fixing);
    }

    #[test]
    fn final_review_runs_after_a_fix_and_an_unreadable_answer_is_a_failed_run() {
        let Some(s) = setup() else { return };
        let (flow, _) = flow_with(&s, false, happy("Mọi thứ ổn."));
        let ctx = s.ctx(1, &Config::default());
        flow.started(&ctx, None).unwrap();
        let mut d = run(&flow, &ctx, FlowAction::Run);
        let mut fixed = false;
        for _ in 0..14 {
            let action = match (d.status, d.gate.as_ref().map(|g| g.kind)) {
                (FlowStatus::Gate, Some(GateKind::Error)) => break,
                (FlowStatus::Gate, Some(GateKind::PlanReview)) => FlowAction::ApprovePlan,
                (FlowStatus::Gate, Some(GateKind::Findings)) if !fixed => {
                    fixed = true;
                    pick_all(&flow, &ctx);
                    FlowAction::PickFindings { fix: true }
                }
                (FlowStatus::Gate, Some(GateKind::Findings)) => FlowAction::PickFindings { fix: false },
                (FlowStatus::Gate, Some(GateKind::AfterFix)) => FlowAction::AfterFix {
                    choice: AfterFix::Next,
                },
                (FlowStatus::Ready, _) => FlowAction::Run,
                other => panic!("{other:?}: {:?}", d.error),
            };
            d = run(&flow, &ctx, action);
        }
        assert!(fixed);
        assert_eq!(d.current, Some(StepKind::FinalReview), "{:?}", d.runs);
        assert_eq!(d.error.as_ref().map(|e| e.code.as_str()), Some("findings.parse"));
        let last = d.runs.last().unwrap();
        assert_eq!((last.step, last.state), (StepKind::FinalReview, RunState::Failed));
    }

    /// The push summary's `## Testing` is built from what the app really ran.
    #[test]
    fn the_push_summary_hears_only_what_really_ran() {
        let mut s = Saved::default();
        assert!(checked_lines(&mut s).is_empty());
        s.checks_note = Some("tsc, eslint: không có lỗi".into());
        s.workflow.set_uses(StepKind::Harness, true);
        s.workflow.set_uses(StepKind::BrowserTest, true);
        let case = |name: &str, ok: bool| harness::Case {
            name: name.into(),
            ok,
            detail: String::new(),
        };
        s.harness = Some(HarnessState {
            passed: false,
            cases: vec![case("2. rỗng", false), case("1. có org", true)],
            runs: 2,
            ..HarnessState::default()
        });
        let shot = |name: &str, ok: bool, untested: bool| browsertest::Case {
            name: name.into(),
            ok,
            detail: String::new(),
            screenshot: None,
            file: None,
            untested,
        };
        s.browser = Some(BrowserState {
            cases: vec![
                shot("1. mở", true, false),
                shot("2. xoá", false, false),
                shot("3. đổi role", false, true),
            ],
            runs: 1,
            ..BrowserState::default()
        });
        let lines = checked_lines(&mut s);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].ends_with("tsc, eslint: không có lỗi"));
        assert!(lines[1].contains("FAIL 1/2; failed: 2. rỗng"), "{}", lines[1]);
        assert!(
            lines[2].contains("OK 1/2; NG: 2. xoá; not tested: 3. đổi role"),
            "{}",
            lines[2]
        );
        // a test step the task turned off says nothing, even with an old result
        s.workflow.set_uses(StepKind::BrowserTest, false);
        assert_eq!(checked_lines(&mut s).len(), 2);
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
        // Tự review, Review chéo, Push summary: Review chốt was passed, the code had not
        // changed since Tự review
        assert_eq!(codex.len(), 3, "{jobs:#?}");
        let final_state = d.steps[StepKind::FinalReview.index() as usize - 1].state;
        assert_eq!(final_state, StepState::Skipped);
        assert!(
            d.log.iter().any(|l| l.text.contains(FINAL_REVIEW_KEPT)),
            "{:?}",
            d.log
        );
        for review in &codex[..2] {
            assert!(review.args.iter().any(|a| a == "--output-schema"));
            assert!(!review.args.iter().any(|a| a == "/review-local"));
            let prompt = review.stdin.as_deref().unwrap();
            assert!(prompt.contains("matching the schema") && prompt.contains("<plan>"));
        }
        assert_eq!(flag(codex[0], "--sandbox").as_deref(), Some("read-only"));
        // each review keeps its own answer file
        let out = |j: &Job| flag(j, "-o").unwrap();
        assert!(out(codex[0]).ends_with("codex-reviewLocal.json"));
        assert!(out(codex[1]).ends_with("codex-r1.json"));
        let push_job = codex[2];
        assert!(push_job.args.iter().any(|a| a == "--ephemeral"));
        let push_prompt = push_job.stdin.as_deref().unwrap();
        assert!(push_prompt.contains("(#279)") && push_prompt.contains("<plan>"));
        assert!(push_prompt.contains("What the app ran on this change:\n- "));
        // asked for by hand, Review chốt runs on the same code, in the mode Settings picked
        let again = FlowAction::Reopen {
            step: Some(StepKind::FinalReview),
            note: None,
        };
        flow.act(ctx.clone(), again).unwrap();
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.status, FlowStatus::AwaitingPush, "{:?}", d.error);
        let jobs = fake.jobs.lock().unwrap().clone();
        let codex: Vec<&Job> = jobs.iter().filter(|j| j.agent == Provider::Codex).collect();
        let final_review = codex[3];
        assert_eq!(
            flag(final_review, "--sandbox").as_deref(),
            Some("workspace-write")
        );
        assert!(out(final_review).ends_with("codex-finalReview.json"));
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
                // `npx eslint {files}` from config: `--` added before the tool (npm 11)
                assert_eq!(check.args, vec!["--", "eslint", "a.ts"]);
                Ok(if n == 0 {
                    (
                        Some(1),
                        // line 2 is the one the fake coder added (errors on other lines are old)
                        "a.ts\n  2:1  error  'x' is unused  no-unused-vars\n".to_owned(),
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
            fix.contains("<check_output>\n### npx eslint {files}\na.ts: 2:1  error  'x' is unused"),
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
        let review_calls = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, stop, out| match kind(job) {
            "codex"
                if job
                    .stdin
                    .as_deref()
                    .is_some_and(|p| p.contains("Independently review the harness")) =>
            {
                assert!(job.args.windows(2).any(|a| a == ["--sandbox", "read-only"]));
                assert!(job.args.iter().any(|a| a == "--output-schema"));
                assert!(!job.args.iter().any(|a| a == "resume"));
                let text = if review_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    "malformed".to_owned()
                } else {
                    serde_json::json!({"findings":[{
                        "severity":"high","file":"run.js","line":1,"title":"Expected value mismatch",
                        "detail":"The expected value disagrees with the brief.","suggestion":"Use the brief."
                    }]})
                    .to_string()
                };
                out(
                    &serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":text}})
                        .to_string(),
                );
                ok()
            }
            _ if job
                .stdin
                .as_deref()
                .is_some_and(|p| p.starts_with("Fix only the selected harness")) =>
            {
                assert!(
                    job.args
                        .windows(2)
                        .any(|a| a[0] == "--add-dir" && Path::new(&a[1]) == job.cwd)
                );
                assert!(job.cwd.join("run.js").is_file());
                std::fs::write(job.cwd.join("run.js"), "// reviewed harness").unwrap();
                out(&result_line("Đã sửa.\n```json\n{\"fixed\":[1]}\n```"));
                ok()
            }
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

        // A review parse failure retries only the review, before the app runs any cases.
        let d = run(&flow, &ctx, FlowAction::Run);
        assert!(d.error.is_some(), "{:?}", d.gate);
        assert!(runs.lock().unwrap().is_empty());
        let d = run(&flow, &ctx, FlowAction::Retry);
        assert_eq!(
            d.gate.as_ref().map(|g| g.kind),
            Some(GateKind::Findings),
            "{:?}",
            d.error
        );
        let review = d.findings.iter().find(|f| f.source == HARNESS_REVIEW).unwrap();
        assert!(runs.lock().unwrap().is_empty());
        flow.decide(&FindingDecision {
            wp_id: 1,
            fingerprint: review.fingerprint.clone(),
            decision: Decision::Fix,
            reason: None,
        })
        .unwrap();
        assert!(
            flow.detail(&ctx)
                .findings
                .iter()
                .any(|f| f.source == HARNESS_REVIEW && f.fix_harness)
        );

        // Repair the review finding in scratch, then the app runs the harness; one case fails.
        let d = run(&flow, &ctx, FlowAction::PickFindings { fix: true });
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
        for (decision, fix_harness) in [(Decision::Fix, false), (Decision::FixHarness, true)] {
            flow.decide(&FindingDecision {
                wp_id: 1,
                fingerprint: f.fingerprint.clone(),
                decision,
                reason: None,
            })
            .unwrap();
            assert!(
                flow.detail(&ctx)
                    .findings
                    .iter()
                    .any(|item| item.fingerprint == f.fingerprint
                        && item.chosen
                        && item.fix_harness == fix_harness)
            );
        }
        let jobs = fake.jobs.lock().unwrap().clone();
        let prompt = jobs
            .iter()
            .find(|j| kind(j) == "harness")
            .unwrap()
            .stdin
            .clone()
            .unwrap();
        assert!(prompt.contains(&library.to_string_lossy().into_owned()) && prompt.contains("- org rỗng"));
        let scratch = PathBuf::from(&runs.lock().unwrap()[0]);
        assert!(!scratch.starts_with(&s.repo) && scratch.join("run.js").is_file());
        assert!(library.join(harness::README).is_file());
        assert!(s.tasks.join("WP-1").join("harness").join("run-1.log").is_file());

        // The user chose to repair the harness; the app reruns it by itself and saves the pass.
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
        assert_eq!(
            harness_runs.len(),
            3,
            "one build and two review attempts; app runs add no AI rows"
        );
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
                .filter(|j| j
                    .stdin
                    .as_deref()
                    .is_some_and(|p| p.starts_with("Build and run a throwaway test harness")))
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
            proposed.commands,
            [DevCommand {
                command: "npm run dev".into(),
                dir: "web".into(),
                wait: String::new(),
            }]
        );
        assert_eq!(d.dev_server, None);
        let set = |command: &str, url: &str, dir: &str| FlowAction::SetDevServer {
            url: url.into(),
            commands: if command.is_empty() {
                Vec::new()
            } else {
                vec![DevCommand {
                    command: command.into(),
                    dir: dir.into(),
                    wait: String::new(),
                }]
            },
            how_to: String::new(),
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
        // with how to reach the screen: it goes into the browser test's prompt
        let how_to = "Mở /files bằng tài khoản test";
        let with_how = match set(" npm run dev ", "http://localhost:5173", "web/") {
            FlowAction::SetDevServer { url, commands, .. } => FlowAction::SetDevServer {
                url,
                commands,
                how_to: format!(" {how_to} "),
            },
            _ => unreachable!(),
        };
        flow.act(ctx.clone(), with_how).unwrap();
        let d = flow.detail(&ctx);
        let proposed = DevServer {
            how_to: how_to.into(),
            ..proposed
        };
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
        assert!(
            prompt.contains("<how_to>\nMở /files bằng tài khoản test\n</how_to>"),
            "{prompt}"
        );
        // the MCP server may write screenshots into the task folder
        assert!(mcp.contains("--filesystem-root"), "{mcp}");
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
        // its row in Tiến trình says "dừng, chờ bạn", not "xong"
        let asked = d.runs.iter().rfind(|r| r.step == StepKind::BrowserTest).unwrap();
        assert_eq!(asked.state, RunState::Waiting);
        assert!(
            d.log
                .iter()
                .any(|l| l.text.starts_with("Test trình duyệt dừng, chờ bạn: ")),
            "{:?}",
            d.log
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
        // answered: that run is over now, the next one finished the test
        assert_eq!(
            (run_rows[0].state, run_rows[1].state),
            (RunState::Done, RunState::Done)
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
                let first = match n {
                    0 => format!("{{\"name\":\"1. chưa có org\",\"ok\":false,\"detail\":\"trang trắng\",\"screenshot\":\"{shot}\",\"file\":\"a.ts\"}}"),
                    1 => "{\"name\":\"1. Mở /files khi chưa có org\",\"ok\":false,\"untested\":true,\"detail\":\"không có user chưa có org\"}".to_owned(),
                    _ => "{\"name\":\"1. chưa có org\",\"ok\":true}".to_owned(),
                };
                let changes = if n == 1 {
                    "\"Thêm người nhận A (đã bỏ lại)\""
                } else {
                    ""
                };
                out(&result_line(&format!(
                    "```json\n{{\"result\":\"ng\",\"summary\":\"thử 2 ca\",\"url\":\"http://localhost:5173/files\",\"cases\":[\
                     {first},{{\"name\":\"2. có org\",\"ok\":true}}],\"changes\":[{changes}]}}\n```"
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
            .find(|f| f.title == "Test trình duyệt NG: 1. chưa có org")
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
        // answering the gate starts the browser test at once; case 1 could not be tested this
        // time: no NG (Review chốt, step 11, is next), but the fix is not called done either
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
        assert!(!view.passed && view.runs == 2);
        assert!(view.cases[0].untested && view.cases[0].detail == "không có user chưa có org");
        assert_eq!(view.changes, ["Thêm người nhận A (đã bỏ lại)"]);
        let browser_findings = |d: &TaskDetail| -> Vec<(String, opw_domain::finding::FindingState)> {
            d.findings
                .iter()
                .filter(|f| f.source == "browserTest")
                .map(|f| (f.title.clone(), f.state))
                .collect()
        };
        assert_eq!(
            browser_findings(&d),
            [(
                "Test trình duyệt NG: 1. chưa có org".to_owned(),
                opw_domain::finding::FindingState::Open
            )]
        );
        let note = d
            .runs
            .iter()
            .rev()
            .find(|r| r.step == StepKind::BrowserTest)
            .and_then(|r| r.note.clone());
        assert_eq!(note.as_deref(), Some("OK 1/1, 1 chưa test, đổi 1 dữ liệu thật"));
        assert!(d.log.iter().any(|l| l.text == "Chưa test được: 1. Mở /files khi chưa có org (không có user chưa có org)"));
        assert!(
            d.log
                .iter()
                .any(|l| l.text == "AI đã thay đổi dữ liệu thật: Thêm người nhận A (đã bỏ lại)")
        );

        // Làm tiếp from Test trình duyệt with a note: it reaches the prompt; case 1 is OK now
        let mut d = run(
            &flow,
            &ctx,
            FlowAction::Reopen {
                step: Some(StepKind::BrowserTest),
                note: Some("Chỉ test lại ca 1".into()),
            },
        );
        if d.status == FlowStatus::Ready && d.current == Some(StepKind::BrowserTest) {
            d = run(&flow, &ctx, FlowAction::Run);
        }
        assert_eq!(
            (d.current, d.status),
            (Some(StepKind::FinalReview), FlowStatus::Ready),
            "{:?}",
            d.error
        );
        let jobs = fake.jobs.lock().unwrap().clone();
        let last = jobs.iter().rev().find(|j| kind(j) == "browser").unwrap();
        assert!(
            last.stdin
                .as_deref()
                .unwrap()
                .contains("<user_note>\nChỉ test lại ca 1\n</user_note>")
        );
        let view = d.tests.iter().find(|t| t.step == StepKind::BrowserTest).unwrap();
        assert!(view.passed && view.runs == 3 && view.changes.is_empty());
        // a case that failed before and passes now is fixed
        assert_eq!(
            browser_findings(&d),
            [(
                "Test trình duyệt NG: 1. chưa có org".to_owned(),
                opw_domain::finding::FindingState::Fixed
            )]
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
        let flow = Flow::new_without_mcp(s.store.clone(), fake.clone(), None);
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
        let flow = Flow::new_without_mcp(s.store.clone(), fake, None);
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
    fn step_advice_turns_optional_steps_on_until_the_user_picks() {
        let Some(s) = setup() else { return };
        let analyses = AtomicUsize::new(0);
        let (flow, fake) = flow_with(&s, false, move |job, _stop, out| {
            if kind(job) == "analyze" {
                let n = analyses.fetch_add(1, Ordering::SeqCst);
                let json = if n == 0 {
                    r#"{"harness":{"recommend":false,"reason":"ít logic"},
                       "browserTest":{"recommend":true,"reason":"đổi màn hình","scenario":["mở trang","bấm Lưu"]}}"#
                } else {
                    r#"{"harness":{"recommend":true,"reason":"có hàm tính","checks":"tổng rỗng"},
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
        assert_eq!(d.proposals.len(), 2);
        assert_eq!(d.proposals[1].checks, "- mở trang\n- bấm Lưu");
        // the AI's advice decides until the user switches a step
        assert_eq!(d.uses, vec![StepKind::BrowserTest]);
        let analyze = d.runs.iter().find(|r| r.step == StepKind::Analyze).unwrap();
        assert_eq!(analyze.note.as_deref(), Some("plan: đề xuất Test trình duyệt"));

        let set = |step, on| FlowAction::SetUses { step, on };
        assert_eq!(
            flow.act(ctx.clone(), set(StepKind::Checks, true))
                .unwrap_err()
                .code,
            "flow.uses"
        );
        flow.act(ctx.clone(), set(StepKind::BrowserTest, false)).unwrap();

        // asking for changes: the new analysis hears the feedback
        let d = run(
            &flow,
            &ctx,
            FlowAction::RejectPlan {
                feedback: "ngắn hơn".into(),
            },
        );
        let jobs = fake.jobs.lock().unwrap().clone();
        let again = jobs[1].stdin.as_deref().unwrap();
        assert!(again.contains("<feedback>\nngắn hơn\n</feedback>"), "{again}");
        assert!(!again.contains("<answers>"));
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
        assert!(!d.plan.as_deref().unwrap().contains("## Câu hỏi và trả lời"));
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
        let flow = Flow::new_without_mcp(s.store.clone(), fake.clone(), Some(runs_dir.clone()));
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
        // a plan with no advice block (a question left in it does not hold the gate)
        assert!(d.uses.is_empty());
        assert_eq!(d.proposals.len(), 1);
        assert!(!d.proposals[0].recommend && !d.proposals[0].reason.is_empty());
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(d.current, Some(StepKind::ReviewLocal), "{:?}", d.error);
        assert!(!d.plan.as_deref().unwrap().contains("## Câu hỏi và trả lời"));
        let [analyze, implement] = &d.runs[..] else {
            panic!("{:?}", d.runs)
        };
        assert_eq!((analyze.step, analyze.state), (StepKind::Analyze, RunState::Done));
        assert_eq!(analyze.note.as_deref(), Some("plan: 2 file sẽ sửa"));
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
        // Tự review goes on in the code session, read only, with the task's brief and plan
        assert_eq!(
            d.runs[2].note.as_deref(),
            Some("không thấy lỗi (trong phiên Viết code)")
        );
        let jobs = fake.jobs.lock().unwrap().clone();
        let review = &jobs[2];
        assert_eq!(flag(review, "--resume").as_deref(), Some("s-1"));
        assert_eq!(flag(review, "--tools").as_deref(), Some("Read,Grep,Glob,Bash"));
        let prompt = review.stdin.as_deref().unwrap();
        assert!(prompt.contains("<plan>") && prompt.contains("a step of the plan left out"));
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
            plan_note("## Nguyên nhân\nkhông có danh sách", &[]),
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
        assert_eq!(plan_note("x", &advice), "plan: đề xuất Harness");
        let job = Job {
            agent: Provider::Claude,
            args: claude::review_local_args("main", 1.0, Some("s-1"), None),
            cwd: PathBuf::new(),
            stdin: None,
            timeout: Duration::from_secs(1),
            mcp_token: None,
        };
        let prompt = prompt_of(&job);
        assert!(prompt.starts_with("/review-local Non-interactive run."));
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
