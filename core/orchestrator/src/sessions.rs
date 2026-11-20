//! Workspace sessions (PLAN.md §4, §12 B): each one is a normal Claude Code session the app runs
//! the way the VS Code extension 2.1.32x does (stream-json in and out, permission requests over
//! stdio, the user's own settings; checked in the A2 probe). The app adds only its git guard.
//!
//! One process per session while it is used; what was said lives in the CLI's transcript, so a
//! session opened again (here, in VS Code or in a terminal) shows the whole conversation. A
//! session open somewhere else is only shown here until it is closed there.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use opw_agents::claude::{DISALLOWED_GIT, cut};
use opw_agents::cli;
use opw_agents::env::{Account, is_stripped};
use opw_agents::session_log::{self, SessionLog};
use opw_agents::transcript::claude_tool_text;
use opw_domain::session::{
    AskAnswer, AskKind, AskOption, AskQuestion, MODES, SessionAsk, SessionChat, SessionKind, SessionPatch,
    SessionState, SessionView,
};
use opw_domain::view::ApiError;
use opw_storage::{SessionRow, Store};
use serde_json::{Value, json};

/// Sessions the app runs say so (`entrypoint` in the transcript and the status file). Without it
/// the CLI writes `sdk-cli`, and the VS Code extension leaves those out of its session list;
/// `cli` is turned back into `sdk-cli` when there is no terminal.
pub const ENTRYPOINT: &str = "op-workflow";
/// A session with nothing to do for this long gives its process back (the next message resumes).
const IDLE_CLOSE: Duration = Duration::from_secs(30 * 60);
/// How often the janitor looks.
const JANITOR_EVERY: Duration = Duration::from_secs(30);
/// Live text is pushed at most this often.
const PARTIAL_EVERY: Duration = Duration::from_millis(80);
/// After stdin closes, how long the CLI has to exit before its tree is killed.
const EXIT_WAIT: Duration = Duration::from_secs(5);
/// VS Code needs a moment to open the folder's window before the session link goes to it.
const VSCODE_DELAY: Duration = Duration::from_secs(3);
const STDERR_KEEP: usize = 8 * 1024;
/// Longest tool input shown in a permission box.
const SUMMARY_MAX: usize = 1500;
/// Longest first message kept as the title.
const TITLE_MAX: usize = 80;
/// The tail of a transcript searched for the title the CLI gives a session.
const TITLE_TAIL: u64 = 64 * 1024;
const TRUST_ASK: &str = "trust";
const DENY_STOP: &str = "The user declined this action. Stop and wait for their instructions.";
const PLAN_AGAIN: &str = "The user did not approve the plan. Keep planning and present a new plan.";

pub type PatchSink = Arc<dyn Fn(SessionPatch) + Send + Sync>;

fn err(code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(code, message)
}

fn now() -> String {
    opw_quota::now_rfc3339()
}

/// How the next process of a session treats its folder's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TrustRun {
    /// Claude Code trusts the folder already.
    Trusted,
    /// The user said yes: record trust for `directory` in `initialize`, as the extension does.
    Attest(String),
    /// The user said no: only the user's own settings.
    UserOnly,
}

struct Pending {
    view: SessionAsk,
    input: Value,
    suggestions: Value,
    tool_use_id: Option<String>,
}

struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    pid: u32,
    run_no: u64,
    stderr: Arc<Mutex<String>>,
}

struct Entry {
    row: SessionRow,
    task_id: Option<String>,
    branch: Option<String>,
    log: SessionLog,
    transcript: Option<PathBuf>,
    /// Modified time of the transcript when it was read.
    read_at: Option<SystemTime>,
    started: bool,
    partial: String,
    partial_sent: Instant,
    asks: Vec<Pending>,
    proc: Option<Proc>,
    /// The last process the app ran for it: its status file can outlive it by a moment, and is
    /// not "open somewhere else".
    last_pid: Option<u32>,
    /// The mode the CLI said it runs in.
    live_mode: Option<String>,
    /// Messages sent whose turn has not ended.
    open_turns: u32,
    last_activity: Instant,
    seq: u64,
    error: Option<String>,
    /// Messages typed before the folder trust question was answered.
    held: Vec<String>,
    /// set_permission_mode request id → mode, to fall back from a refused `auto`.
    mode_requests: HashMap<String, String>,
    next_req: u64,
}

impl Entry {
    fn new(row: SessionRow, task_id: Option<String>, branch: Option<String>) -> Self {
        Self {
            started: row.cost_usd > 0.0,
            row,
            task_id,
            branch,
            log: SessionLog::default(),
            transcript: None,
            read_at: None,
            partial: String::new(),
            partial_sent: Instant::now(),
            asks: Vec::new(),
            proc: None,
            last_pid: None,
            live_mode: None,
            open_turns: 0,
            last_activity: Instant::now(),
            seq: 0,
            error: None,
            held: Vec::new(),
            mode_requests: HashMap::new(),
            next_req: 0,
        }
    }

    fn kind(&self) -> SessionKind {
        if self.row.kind == "main" {
            SessionKind::Main
        } else {
            SessionKind::Free
        }
    }

    fn title(&self) -> String {
        self.log
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .or_else(|| Some(self.row.title.clone()).filter(|t| !t.is_empty()))
            .unwrap_or_else(|| "Phiên mới".to_owned())
    }

    fn view(&self, held: Option<&Held>) -> SessionView {
        let ours =
            |h: &Held| self.proc.as_ref().is_some_and(|p| p.pid == h.pid) || self.last_pid == Some(h.pid);
        let outside = held.filter(|h| !ours(h)).map(|h| where_open(&h.entrypoint));
        let state = if !self.asks.is_empty() {
            SessionState::Waiting
        } else if self.proc.is_some() && self.open_turns > 0 {
            SessionState::Running
        } else if outside.is_some() {
            SessionState::Outside
        } else {
            SessionState::Idle
        };
        SessionView {
            id: self.row.id.clone(),
            kind: self.kind(),
            wp_id: self.row.wp_id,
            task_id: self.task_id.clone(),
            title: self.title(),
            repo: self.row.repo.clone(),
            branch: self.branch.clone(),
            state,
            live: self.proc.is_some(),
            outside,
            mode: self.live_mode.clone().or_else(|| self.row.mode.clone()),
            model: self.row.model.clone(),
            cost_usd: self.row.cost_usd,
            started: self.started,
            created_at: self.row.created_at.clone(),
            updated_at: self.row.updated_at.clone(),
            error: self.error.clone(),
        }
    }

    fn chat(&self, held: Option<&Held>) -> SessionChat {
        SessionChat {
            session: self.view(held),
            items: self
                .log
                .items()
                .iter()
                .cloned()
                .map(crate::flow::chat_item)
                .collect(),
            dropped: u32::try_from(self.log.dropped()).unwrap_or(u32::MAX),
            partial: self.partial.clone(),
            asks: self.asks.iter().map(|p| p.view.clone()).collect(),
            seq: self.seq,
        }
    }

    fn write(&mut self, v: &Value) -> Result<(), ApiError> {
        let stdin = self
            .proc
            .as_mut()
            .and_then(|p| p.stdin.as_mut())
            .ok_or_else(|| err("session.stopped", "Phiên không còn chạy"))?;
        let line = format!("{v}\n");
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| err("session.write", format!("Không gửi được vào phiên: {e}")))
    }

    fn control(&mut self, body: Value) -> Result<String, ApiError> {
        self.next_req += 1;
        let rid = format!("opw_{}", self.next_req);
        self.write(&json!({ "type": "control_request", "request_id": rid, "request": body }))?;
        Ok(rid)
    }

    fn request_mode(&mut self, mode: &str) -> Result<(), ApiError> {
        let rid = self.control(json!({ "subtype": "set_permission_mode", "mode": mode }))?;
        self.mode_requests.insert(rid, mode.to_owned());
        Ok(())
    }

    fn say(&mut self, text: &str) -> Result<(), ApiError> {
        self.write(&json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
            "parent_tool_use_id": null,
            "session_id": "",
        }))?;
        self.open_turns += 1;
        self.last_activity = Instant::now();
        self.started = true;
        if self.row.title.is_empty() {
            self.row.title = first_line(text, TITLE_MAX);
        }
        Ok(())
    }

    /// Read the transcript again when it changed and nothing here writes to it.
    fn refresh(&mut self) {
        if self.proc.is_some() {
            return;
        }
        if self.transcript.as_ref().is_none_or(|p| !p.is_file()) {
            self.transcript = transcript_path(&self.row.repo, &self.row.id);
        }
        let Some(path) = self.transcript.clone() else {
            return;
        };
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if modified.is_some() && modified == self.read_at {
            return;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        self.log = SessionLog::from_transcript(&text);
        self.read_at = modified;
        self.started = true;
        self.seq += 1;
        if let Some(title) = self.log.title.clone() {
            self.row.title = title;
        }
    }

    fn clear_turn(&mut self) {
        self.asks.clear();
        self.partial.clear();
        self.open_turns = 0;
    }
}

/// A session's status file (`~/.claude/sessions/<pid>.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    pid: u32,
    status: String,
    entrypoint: String,
}

fn where_open(entrypoint: &str) -> String {
    match entrypoint {
        "claude-vscode" => "VS Code".to_owned(),
        "cli" => "terminal".to_owned(),
        "claude-desktop" => "Claude Desktop".to_owned(),
        // the dev build next to the installed app, or the other way round
        ENTRYPOINT => "một op-workflow khác".to_owned(),
        "" => "nơi khác".to_owned(),
        other => other.to_owned(),
    }
}

fn claude_home() -> Option<PathBuf> {
    crate::browsertest::claude_projects().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Every session some Claude Code process holds now, by session id.
fn status_files() -> HashMap<String, Held> {
    let mut out = HashMap::new();
    let Some(dir) = claude_home().map(|h| h.join("sessions")) else {
        return out;
    };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        if let Some((id, held)) = std::fs::read_to_string(&path).ok().and_then(|t| parse_status(&t)) {
            out.insert(id, held);
        }
    }
    out
}

fn parse_status(text: &str) -> Option<(String, Held)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let id = v["sessionId"].as_str()?.to_owned();
    let pid = u32::try_from(v["pid"].as_u64()?).ok()?;
    Some((
        id,
        Held {
            pid,
            status: v["status"].as_str().unwrap_or_default().to_owned(),
            entrypoint: v["entrypoint"].as_str().unwrap_or_default().to_owned(),
        },
    ))
}

fn status_of_pid(pid: u32) -> Option<String> {
    let path = claude_home()?.join("sessions").join(format!("{pid}.json"));
    parse_status(&std::fs::read_to_string(path).ok()?).map(|(_, h)| h.status)
}

/// The folder Claude Code keeps a project's transcripts in: every character that is not a
/// letter or a digit becomes `-`.
fn project_dir_name(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn transcript_path(repo: &str, id: &str) -> Option<PathBuf> {
    let projects = crate::browsertest::claude_projects()?;
    let file = format!("{id}.jsonl");
    let repo = opw_config::expand_home(repo).to_string_lossy().into_owned();
    let guess = projects.join(project_dir_name(&repo)).join(&file);
    if guess.is_file() {
        return Some(guess);
    }
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(&file))
        .find(|p| p.is_file())
}

/// The title the CLI gave the session, from the end of its transcript.
fn title_from_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TITLE_TAIL))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    String::from_utf8_lossy(&buf)
        .lines()
        .rev()
        .filter(|l| l.contains("\"ai-title\"") || l.contains("\"custom-title\""))
        .find_map(|l| {
            let v: Value = serde_json::from_str(l).ok()?;
            v["customTitle"]
                .as_str()
                .or_else(|| v["aiTitle"].as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        })
}

fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= max {
        line.to_owned()
    } else {
        format!("{}…", line.chars().take(max).collect::<String>())
    }
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut cmd);
        let _ = cmd.status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

/// Close stdin the way the SDK ends a query, then kill the tree if the CLI does not exit.
fn close_proc(mut proc: Proc, wait: Duration) {
    drop(proc.stdin.take());
    let start = Instant::now();
    loop {
        match proc.child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if start.elapsed() < wait => std::thread::sleep(Duration::from_millis(100)),
            _ => break,
        }
    }
    kill_tree(proc.pid);
    let _ = proc.child.wait();
}

fn stripped_env() -> Vec<String> {
    std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| is_stripped(k, Account::Subscription))
        .collect()
}

/// `claude --setting-sources user workspace-trust --json` in `repo`, as the extension asks before
/// a chat: `{trusted, directory, root?}`. No tokens.
fn trust_report(repo: &Path) -> Option<Value> {
    let exe = cli::find_claude()?;
    let mut cmd = Command::new(exe);
    cmd.args(["--setting-sources", "user", "workspace-trust", "--json"])
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for k in stripped_env() {
        cmd.env_remove(k);
    }
    hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().rev().find_map(|l| {
        serde_json::from_str::<Value>(l.trim())
            .ok()
            .filter(Value::is_object)
    })
}

/// The arguments of a session process: the extension's flags (2.1.32x, trusted folder) and the
/// app's git guard.
fn session_args(id: &str, resume: bool, mode: Option<&str>, user_only: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
        "--permission-prompt-tool=stdio",
        "--include-partial-messages",
        "--replay-user-messages",
        "--disallowedTools",
        DISALLOWED_GIT,
        "--setting-sources",
        if user_only { "user" } else { "user,project,local" },
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    if let Some(mode) = mode {
        args.extend(["--permission-mode".to_owned(), mode.to_owned()]);
    }
    args.extend([
        if resume { "--resume" } else { "--session-id" }.to_owned(),
        id.to_owned(),
    ]);
    args
}

/// What a permission request asks, for the chat.
fn ask_view(rid: &str, req: &Value) -> SessionAsk {
    let tool = req["tool_name"].as_str().unwrap_or("?").to_owned();
    let input = &req["input"];
    let kind = match tool.as_str() {
        "AskUserQuestion" => AskKind::Question,
        "ExitPlanMode" => AskKind::Plan,
        _ => AskKind::Permission,
    };
    let detail = [
        req["description"].as_str(),
        req["decision_reason"].as_str(),
        input["description"].as_str(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|s| !s.is_empty())
    .map(|s| cut(s, 400));
    let always = (kind == AskKind::Permission && req["suppress_always_allow_rule"] != true)
        .then(|| describe_suggestions(&req["permission_suggestions"]))
        .flatten();
    let questions = input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|q| AskQuestion {
            question: q["question"].as_str().unwrap_or_default().to_owned(),
            header: q["header"].as_str().unwrap_or_default().to_owned(),
            options: q["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| AskOption {
                    label: o["label"].as_str().unwrap_or_default().to_owned(),
                    description: o["description"].as_str().unwrap_or_default().to_owned(),
                })
                .collect(),
            multi_select: q["multiSelect"] == true,
        })
        .collect();
    SessionAsk {
        id: rid.to_owned(),
        kind,
        summary: if kind == AskKind::Permission {
            cut(&claude_tool_text(&tool, input), SUMMARY_MAX)
        } else {
            String::new()
        },
        detail: if kind == AskKind::Permission { detail } else { None },
        always,
        questions,
        plan: (kind == AskKind::Plan).then(|| input["plan"].as_str().unwrap_or_default().to_owned()),
        tool,
    }
}

/// "Luôn cho phép" in words: the rules / folders the CLI suggests and where they would be kept.
fn describe_suggestions(s: &Value) -> Option<String> {
    let list = s.as_array().filter(|a| !a.is_empty())?;
    let mut parts: Vec<String> = Vec::new();
    for item in list {
        let what = match item["type"].as_str().unwrap_or_default() {
            "addRules" | "replaceRules" => item["rules"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| match r["ruleContent"].as_str() {
                    Some(c) if !c.is_empty() => format!("{}({c})", r["toolName"].as_str().unwrap_or("?")),
                    _ => r["toolName"].as_str().unwrap_or("?").to_owned(),
                })
                .collect::<Vec<_>>()
                .join(", "),
            "addDirectories" => format!(
                "thư mục {}",
                item["directories"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "setMode" => format!("mode {}", item["mode"].as_str().unwrap_or("?")),
            _ => continue,
        };
        let place = match item["destination"].as_str().unwrap_or_default() {
            "session" => "trong phiên này",
            "localSettings" => "lưu vào .claude/settings.local.json của repo",
            "projectSettings" => "lưu vào .claude/settings.json của repo",
            "userSettings" => "lưu vào settings của bạn",
            _ => "",
        };
        parts.push(if place.is_empty() {
            what
        } else {
            format!("{what} · {place}")
        });
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The `can_use_tool` answer for `answer` (`None` = it does not fit this ask).
fn tool_response(p: &Pending, answer: &AskAnswer) -> Option<Value> {
    let mut r = match (p.view.kind, answer) {
        (AskKind::Permission, AskAnswer::Allow { always }) => {
            let mut r = json!({ "behavior": "allow", "updatedInput": p.input });
            if *always && p.suggestions.as_array().is_some_and(|a| !a.is_empty()) {
                r["updatedPermissions"] = p.suggestions.clone();
            }
            r
        }
        (AskKind::Permission | AskKind::Question, AskAnswer::Deny { message }) => {
            match message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                // a reason: the AI reads it and goes on
                Some(m) => json!({ "behavior": "deny", "message": m, "interrupt": false }),
                // a plain no: stop and wait, as "No" does in the terminal
                None => json!({ "behavior": "deny", "message": DENY_STOP, "interrupt": true }),
            }
        }
        (AskKind::Question, AskAnswer::Answers { answers }) => {
            let mut updated = p.input.clone();
            updated["answers"] = json!(answers);
            json!({ "behavior": "allow", "updatedInput": updated })
        }
        (AskKind::Plan, AskAnswer::ApprovePlan { .. }) => {
            json!({ "behavior": "allow", "updatedInput": p.input })
        }
        (AskKind::Plan, AskAnswer::RejectPlan { feedback }) => {
            let m = feedback
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map_or_else(
                    || PLAN_AGAIN.to_owned(),
                    |f| format!("{PLAN_AGAIN}\n\nFeedback: {f}"),
                );
            json!({ "behavior": "deny", "message": m, "interrupt": false })
        }
        _ => return None,
    };
    if let Some(id) = &p.tool_use_id {
        r["toolUseID"] = json!(id);
    }
    Some(r)
}

struct Inner {
    store: Arc<Mutex<Store>>,
    hooks_dir: PathBuf,
    entries: Mutex<HashMap<String, Arc<Mutex<Entry>>>>,
    sink: Mutex<Option<PatchSink>>,
    /// repo → the user's trust answer this run (`true` = trusted / attested).
    trust: Mutex<HashMap<String, bool>>,
    run_no: AtomicU64,
}

/// The sessions the Workspace shows. Cheap to clone.
#[derive(Clone)]
pub struct Sessions {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions")
            .field("open", &lock(&self.inner.entries).len())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Sessions {
    pub fn new(store: Arc<Mutex<Store>>, hooks_dir: PathBuf) -> Self {
        let _ = opw_vcs::install_guard_hooks(&hooks_dir);
        Self {
            inner: Arc::new(Inner {
                store,
                hooks_dir,
                entries: Mutex::new(HashMap::new()),
                sink: Mutex::new(None),
                trust: Mutex::new(HashMap::new()),
                run_no: AtomicU64::new(0),
            }),
        }
    }

    /// Where pushed changes go (the shell turns them into a Tauri event).
    pub fn set_sink(&self, sink: PatchSink) {
        *lock(&self.inner.sink) = Some(sink);
    }

    /// Closes sessions idle for [`IDLE_CLOSE`] and settles turns the CLI ended quietly.
    pub fn start_janitor(&self) {
        let weak = Arc::downgrade(&self.inner);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(JANITOR_EVERY);
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                Sessions { inner }.tidy();
            }
        });
    }

    fn tidy(&self) {
        for (id, arc) in self.all_entries() {
            let mut e = lock(&arc);
            let Some(pid) = e.proc.as_ref().map(|p| p.pid) else {
                continue;
            };
            if !e.asks.is_empty() {
                continue;
            }
            if e.open_turns > 0 {
                // a queued message the CLI folded into the running turn leaves one turn open
                if e.last_activity.elapsed() > Duration::from_secs(10)
                    && status_of_pid(pid).as_deref() == Some("idle")
                {
                    e.open_turns = 0;
                    self.emit(&id, &mut e);
                }
            } else if e.last_activity.elapsed() > IDLE_CLOSE {
                if let Some(proc) = e.proc.take() {
                    std::thread::spawn(move || close_proc(proc, EXIT_WAIT));
                }
                self.emit(&id, &mut e);
            }
        }
    }

    fn all_entries(&self) -> Vec<(String, Arc<Mutex<Entry>>)> {
        lock(&self.inner.entries)
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.inner.store)
    }

    fn save(&self, e: &mut Entry) {
        e.row.updated_at = now();
        let _ = self.store().save_session(&e.row);
    }

    /// Push a change of `e` to the UI: the items from the first one changed, live text, asks.
    fn emit(&self, id: &str, e: &mut Entry) {
        e.seq += 1;
        let Some(sink) = lock(&self.inner.sink).clone() else {
            let _ = e.log.take_changed();
            return;
        };
        let len = e.log.items().len();
        let from = e.log.take_changed().unwrap_or(len).min(len);
        let patch = SessionPatch {
            id: id.to_owned(),
            seq: e.seq,
            from: u32::try_from(from).unwrap_or(u32::MAX),
            items: e.log.items()[from..]
                .iter()
                .cloned()
                .map(crate::flow::chat_item)
                .collect(),
            dropped: u32::try_from(e.log.dropped()).unwrap_or(u32::MAX),
            partial: e.partial.clone(),
            asks: e.asks.iter().map(|p| p.view.clone()).collect(),
            session: e.view(None),
        };
        sink(patch);
    }

    /// The session's entry, read from the database the first time.
    fn entry(&self, id: &str) -> Result<Arc<Mutex<Entry>>, ApiError> {
        if let Some(e) = lock(&self.inner.entries).get(id) {
            return Ok(e.clone());
        }
        let row = self
            .store()
            .session(id)
            .map_err(|e| err("storage", e.to_string()))?
            .ok_or_else(|| err("session.missing", "Không còn phiên này"))?;
        Ok(self.insert(row))
    }

    fn insert(&self, row: SessionRow) -> Arc<Mutex<Entry>> {
        let task = row.wp_id.and_then(|wp| self.store().task(wp).ok().flatten());
        let id = row.id.clone();
        let mut entry = Entry::new(
            row,
            task.as_ref().map(|t| t.display_id.clone()),
            task.and_then(|t| t.branch),
        );
        if !entry.started {
            entry.started = transcript_path(&entry.row.repo, &entry.row.id).is_some();
        }
        let arc = Arc::new(Mutex::new(entry));
        lock(&self.inner.entries)
            .entry(id)
            .or_insert_with(|| arc.clone())
            .clone()
    }

    /// Every session on the Workspace list, the most recently used first.
    pub fn list(&self) -> Vec<SessionView> {
        let rows = self.store().sessions().unwrap_or_default();
        let held = status_files();
        rows.into_iter()
            .map(|row| {
                let id = row.id.clone();
                let known = lock(&self.inner.entries).get(&id).cloned();
                let arc = known.unwrap_or_else(|| self.insert(row));
                let e = lock(&arc);
                e.view(held.get(&id))
            })
            .collect()
    }

    /// A new session: task `wp_id`'s (kind main) or one in `repo` without a task.
    pub fn create(&self, wp_id: Option<u64>, repo: &str) -> Result<SessionView, ApiError> {
        let at = now();
        let row = SessionRow {
            id: new_uuid(),
            wp_id,
            repo: repo.to_owned(),
            kind: if wp_id.is_some() { "main" } else { "free" }.to_owned(),
            title: String::new(),
            mode: None,
            model: None,
            cost_usd: 0.0,
            created_at: at.clone(),
            updated_at: at,
        };
        self.store()
            .save_session(&row)
            .map_err(|e| err("storage", e.to_string()))?;
        let arc = self.insert(row);
        let e = lock(&arc);
        Ok(e.view(None))
    }

    /// The newest session of a task, if it has one.
    pub fn task_session(&self, wp_id: u64) -> Option<String> {
        self.store()
            .sessions()
            .ok()?
            .into_iter()
            .filter(|r| r.wp_id == Some(wp_id) && r.kind == "main")
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
            .map(|r| r.id)
    }

    /// Everything the chat pane shows; a session not running here is read from its transcript.
    pub fn chat(&self, id: &str) -> Result<SessionChat, ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        let before = e.row.title.clone();
        e.refresh();
        if e.row.title != before {
            self.save(&mut e);
        }
        let held = status_files();
        Ok(e.chat(held.get(id)))
    }

    /// Send a message: started (or resumed) first when nothing runs. `force` sends even though
    /// the session looks open somewhere else (a stale status file after a crash).
    pub fn send(&self, id: &str, text: &str, force: bool) -> Result<SessionView, ApiError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(err("session.empty", "Tin nhắn trống"));
        }
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        if e.proc.is_none() {
            if !force {
                if let Some(h) = status_files().get(id).filter(|h| e.last_pid != Some(h.pid)) {
                    return Err(err(
                        "session.outside",
                        format!(
                            "Phiên đang mở ở {}. Đóng phiên ở đó rồi gõ tiếp ở đây, hoặc bấm Gõ tiếp ở đây nếu nó đã đóng.",
                            where_open(&h.entrypoint)
                        ),
                    ));
                }
            }
            if e.asks.iter().any(|a| a.view.kind == AskKind::Trust) {
                e.held.push(text.to_owned());
                return Ok(e.view(None));
            }
            e.refresh();
            match self.trust_run(&e.row.repo) {
                Some(run) => self.spawn(id, &mut e, &run)?,
                None => {
                    e.held.push(text.to_owned());
                    let folder = e.row.repo.clone();
                    e.asks.push(Pending {
                        view: SessionAsk {
                            id: TRUST_ASK.to_owned(),
                            kind: AskKind::Trust,
                            tool: String::new(),
                            summary: folder,
                            detail: None,
                            always: None,
                            questions: Vec::new(),
                            plan: None,
                        },
                        input: Value::Null,
                        suggestions: Value::Null,
                        tool_use_id: None,
                    });
                    self.emit(id, &mut e);
                    return Ok(e.view(None));
                }
            }
        }
        e.say(text)?;
        e.error = None;
        self.save(&mut e);
        self.emit(id, &mut e);
        Ok(e.view(None))
    }

    /// The trust a new process runs with, or `None` when the user has to be asked.
    fn trust_run(&self, repo: &str) -> Option<TrustRun> {
        let key = repo.to_lowercase();
        match lock(&self.inner.trust).get(&key) {
            Some(true) => return Some(TrustRun::Trusted),
            Some(false) => return Some(TrustRun::UserOnly),
            None => {}
        }
        // no answer from Claude Code (old CLI, no claude): run as the user would, unasked
        let Some(report) = trust_report(&opw_config::expand_home(repo)) else {
            return Some(TrustRun::Trusted);
        };
        if report["trusted"] == true {
            lock(&self.inner.trust).insert(key, true);
            return Some(TrustRun::Trusted);
        }
        None
    }

    fn spawn(&self, id: &str, e: &mut Entry, trust: &TrustRun) -> Result<(), ApiError> {
        let exe =
            cli::find_claude().ok_or_else(|| err("claude.missing", "Không tìm thấy claude trên PATH"))?;
        let repo = opw_config::expand_home(&e.row.repo);
        if !repo.is_dir() {
            return Err(err(
                "session.repo",
                format!("Không còn thư mục {}", repo.display()),
            ));
        }
        let resume =
            e.transcript.as_ref().is_some_and(|p| p.is_file()) || transcript_path(&e.row.repo, id).is_some();
        let args = session_args(id, resume, e.row.mode.as_deref(), *trust == TrustRun::UserOnly);
        let mut cmd = Command::new(exe);
        cmd.args(&args)
            .current_dir(&repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for k in stripped_env() {
            cmd.env_remove(k);
        }
        cmd.envs(opw_vcs::guard_env(&self.inner.hooks_dir));
        cmd.env("CLAUDE_CODE_ENTRYPOINT", ENTRYPOINT);
        hide_window(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| err("session.spawn", format!("Không mở được claude: {e}")))?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let run_no = self.inner.run_no.fetch_add(1, Ordering::Relaxed) + 1;
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(pipe) = stderr_pipe {
            let keep = stderr.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines() {
                    let Ok(line) = line else { break };
                    let mut s = lock(&keep);
                    s.push_str(&line);
                    s.push('\n');
                    if s.len() > STDERR_KEEP * 2 {
                        let cut_at = s.len() - STDERR_KEEP;
                        let cut_at = (cut_at..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(0);
                        s.drain(..cut_at);
                    }
                }
            });
        }
        if let Some(out) = stdout {
            let me = self.clone();
            let id = id.to_owned();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                        me.on_line(&id, run_no, &v);
                    }
                }
                me.on_exit(&id, run_no);
            });
        }
        e.last_pid = Some(pid);
        e.proc = Some(Proc {
            child,
            stdin,
            pid,
            run_no,
            stderr,
        });
        e.live_mode = None;
        e.last_activity = Instant::now();
        let mut init = json!({ "subtype": "initialize" });
        if let TrustRun::Attest(dir) = trust {
            init["workspaceTrust"] = json!({ "accepted": true, "directory": dir });
        }
        e.control(init)?;
        Ok(())
    }

    fn on_line(&self, id: &str, run_no: u64, v: &Value) {
        let Some(arc) = lock(&self.inner.entries).get(id).cloned() else {
            return;
        };
        let mut e = lock(&arc);
        if e.proc.as_ref().is_none_or(|p| p.run_no != run_no) {
            return;
        }
        e.last_activity = Instant::now();
        match v["type"].as_str().unwrap_or_default() {
            "control_request" => self.on_request(id, &mut e, v),
            "control_cancel_request" => {
                let rid = v["request_id"].as_str().unwrap_or_default();
                e.asks.retain(|a| a.view.id != rid);
                self.emit(id, &mut e);
            }
            "control_response" => self.on_response(id, &mut e, v),
            "stream_event" => {
                if let Some(delta) = session_log::text_delta(v) {
                    e.partial.push_str(delta);
                    if e.partial_sent.elapsed() >= PARTIAL_EVERY {
                        e.partial_sent = Instant::now();
                        self.emit(id, &mut e);
                    }
                }
            }
            "system" => {
                let mut changed = v["subtype"] == "compact_boundary";
                if v["subtype"] == "init" {
                    if let Some(model) = v["model"].as_str() {
                        if e.row.model.as_deref() != Some(model) {
                            e.row.model = Some(model.to_owned());
                            self.save(&mut e);
                            changed = true;
                        }
                    }
                }
                if let Some(mode) = v["permissionMode"].as_str() {
                    if e.live_mode.as_deref() != Some(mode) {
                        e.live_mode = Some(mode.to_owned());
                        changed = true;
                    }
                }
                e.log.apply(v);
                if changed {
                    self.emit(id, &mut e);
                }
            }
            "assistant" => {
                let text = v
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .is_some_and(|b| b.iter().any(|c| c["type"] == "text"));
                if text && v["parent_tool_use_id"].is_null() {
                    e.partial.clear();
                }
                e.log.apply(v);
                self.emit(id, &mut e);
            }
            "user" => {
                e.log.apply(v);
                self.emit(id, &mut e);
            }
            "result" => {
                e.log.apply(v);
                e.open_turns = e.open_turns.saturating_sub(1);
                e.partial.clear();
                if e.open_turns == 0 {
                    e.asks.clear();
                }
                if let Some(total) = v["total_cost_usd"].as_f64() {
                    e.row.cost_usd = e.row.cost_usd.max(total);
                }
                if e.transcript.is_none() {
                    e.transcript = transcript_path(&e.row.repo, id);
                }
                if let Some(title) = e.transcript.as_deref().and_then(title_from_tail) {
                    e.log.title = Some(title.clone());
                    e.row.title = title;
                }
                self.save(&mut e);
                self.emit(id, &mut e);
            }
            _ => {}
        }
    }

    fn on_request(&self, id: &str, e: &mut Entry, v: &Value) {
        let rid = v["request_id"].as_str().unwrap_or_default().to_owned();
        let req = &v["request"];
        match req["subtype"].as_str().unwrap_or_default() {
            "can_use_tool" => {
                e.asks.push(Pending {
                    view: ask_view(&rid, req),
                    input: req["input"].clone(),
                    suggestions: req["permission_suggestions"].clone(),
                    tool_use_id: req["tool_use_id"].as_str().map(str::to_owned),
                });
                self.emit(id, e);
            }
            // an MCP server asks for input the app cannot show: decline, as the SDK does
            "elicitation" => {
                let _ = e.write(&json!({ "type": "control_response", "response": {
                    "subtype": "success", "request_id": rid, "response": { "action": "decline" } } }));
            }
            other => {
                let _ = e.write(&json!({ "type": "control_response", "response": {
                    "subtype": "error", "request_id": rid,
                    "error": format!("op-workflow does not handle {other}") } }));
            }
        }
    }

    fn on_response(&self, id: &str, e: &mut Entry, v: &Value) {
        let r = &v["response"];
        let rid = r["request_id"].as_str().unwrap_or_default();
        let Some(mode) = e.mode_requests.remove(rid) else {
            return;
        };
        if r["subtype"] == "success" {
            e.live_mode = Some(mode);
        } else if mode == "auto" {
            // auto mode is not available on this account: the extension falls back the same way
            let _ = e.request_mode("acceptEdits");
        }
        self.emit(id, e);
    }

    fn on_exit(&self, id: &str, run_no: u64) {
        let Some(arc) = lock(&self.inner.entries).get(id).cloned() else {
            return;
        };
        let mut e = lock(&arc);
        if e.proc.as_ref().is_none_or(|p| p.run_no != run_no) {
            return;
        }
        let Some(mut proc) = e.proc.take() else {
            return;
        };
        let code = proc.child.wait().ok().and_then(|s| s.code());
        if code != Some(0) {
            let tail = lock(&proc.stderr).clone();
            let tail: String = tail
                .lines()
                .rev()
                .filter(|l| !l.trim().is_empty())
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            e.error = Some(if tail.is_empty() {
                format!("claude dừng bất ngờ (mã {code:?})")
            } else {
                format!("claude dừng bất ngờ (mã {code:?}): {}", cut(&tail, 600))
            });
        }
        e.clear_turn();
        self.emit(id, &mut e);
    }

    /// Answer what the session asks.
    pub fn answer(&self, id: &str, ask_id: &str, answer: &AskAnswer) -> Result<SessionView, ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        let at = e
            .asks
            .iter()
            .position(|a| a.view.id == ask_id)
            .ok_or_else(|| err("session.ask", "Câu hỏi này không còn chờ trả lời"))?;
        if e.asks[at].view.kind == AskKind::Trust {
            let AskAnswer::Trust { trust } = answer else {
                return Err(err("session.ask", "Câu trả lời không hợp với câu hỏi"));
            };
            let run = if *trust {
                let report = trust_report(&opw_config::expand_home(&e.row.repo));
                let dir = report
                    .and_then(|r| r["directory"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| {
                        opw_config::expand_home(&e.row.repo)
                            .to_string_lossy()
                            .into_owned()
                    });
                TrustRun::Attest(dir)
            } else {
                TrustRun::UserOnly
            };
            lock(&self.inner.trust).insert(e.row.repo.to_lowercase(), *trust);
            e.asks.remove(at);
            if let Err(error) = self.spawn(id, &mut e, &run) {
                e.error = Some(error.message.clone());
                e.held.clear();
                self.emit(id, &mut e);
                return Err(error);
            }
            for text in std::mem::take(&mut e.held) {
                e.say(&text)?;
            }
            self.save(&mut e);
            self.emit(id, &mut e);
            return Ok(e.view(None));
        }
        let response = tool_response(&e.asks[at], answer)
            .ok_or_else(|| err("session.ask", "Câu trả lời không hợp với câu hỏi"))?;
        if let AskAnswer::ApprovePlan { mode: Some(mode) } = answer {
            if MODES.contains(&mode.as_str()) {
                e.request_mode(mode)?;
            }
        }
        e.write(&json!({ "type": "control_response", "response": {
            "subtype": "success", "request_id": ask_id, "response": response } }))?;
        e.asks.remove(at);
        e.last_activity = Instant::now();
        self.emit(id, &mut e);
        Ok(e.view(None))
    }

    /// Stop the running turn (the session stays open).
    pub fn interrupt(&self, id: &str) -> Result<(), ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        if e.proc.is_none() {
            return Ok(());
        }
        e.control(json!({ "subtype": "interrupt" }))?;
        Ok(())
    }

    /// The permission mode: now if the session runs, and for every later start.
    pub fn set_mode(&self, id: &str, mode: &str) -> Result<SessionView, ApiError> {
        if !MODES.contains(&mode) {
            return Err(err("session.mode", format!("Không có mode {mode}")));
        }
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        e.row.mode = Some(mode.to_owned());
        if e.proc.is_some() {
            e.request_mode(mode)?;
        }
        self.save(&mut e);
        self.emit(id, &mut e);
        Ok(e.view(None))
    }

    /// End the session's process (the next message resumes it).
    pub fn stop(&self, id: &str) -> Result<SessionView, ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        if let Some(proc) = e.proc.take() {
            std::thread::spawn(move || close_proc(proc, EXIT_WAIT));
        }
        e.clear_turn();
        self.emit(id, &mut e);
        Ok(e.view(None))
    }

    /// Give the session to VS Code: end the process here, open the repo's window, then the
    /// session in the extension (`vscode://anthropic.claude-code/open?session=<id>`).
    pub fn open_vscode(&self, id: &str) -> Result<(), ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        if !e.started {
            return Err(err("session.empty", "Phiên chưa có tin nào để mở trong VS Code"));
        }
        let proc = e.proc.take();
        e.clear_turn();
        self.emit(id, &mut e);
        let repo = opw_config::expand_home(&e.row.repo);
        let url = format!("vscode://anthropic.claude-code/open?session={id}");
        drop(e);
        std::thread::spawn(move || {
            if let Some(proc) = proc {
                close_proc(proc, EXIT_WAIT);
            }
            open_in_vscode(&repo, &url);
        });
        Ok(())
    }

    /// Take a session off the list (its transcript stays; the CLI can still resume it).
    pub fn hide(&self, id: &str) -> Result<(), ApiError> {
        let removed = lock(&self.inner.entries).remove(id);
        if let Some(arc) = removed {
            let proc = lock(&arc).proc.take();
            if let Some(proc) = proc {
                std::thread::spawn(move || close_proc(proc, EXIT_WAIT));
            }
        }
        self.store()
            .hide_session(id, &now())
            .map_err(|e| err("storage", e.to_string()))?;
        Ok(())
    }

    /// Sessions with a turn running now (the close dialog counts them).
    pub fn running(&self) -> usize {
        self.all_entries()
            .iter()
            .filter(|(_, arc)| {
                let e = lock(arc);
                e.proc.is_some() && e.open_turns > 0
            })
            .count()
    }

    /// End every process (the app quits); waits at most `wait` before killing what is left.
    pub fn shutdown(&self, wait: Duration) {
        let procs: Vec<Proc> = self
            .all_entries()
            .iter()
            .filter_map(|(_, arc)| {
                let mut e = lock(arc);
                e.clear_turn();
                e.proc.take()
            })
            .collect();
        let handles: Vec<_> = procs
            .into_iter()
            .map(|p| std::thread::spawn(move || close_proc(p, wait)))
            .collect();
        for h in handles {
            let _ = h.join();
        }
    }
}

/// `code <repo>` (the folder's window opens or comes to the front), then the session link.
fn open_in_vscode(repo: &Path, url: &str) {
    if let Some(code) = cli::find_in_path(
        &["code.cmd", "code"],
        &std::env::var_os("PATH").unwrap_or_default(),
    ) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(code).arg(repo);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut cmd);
        let _ = cmd.status();
        std::thread::sleep(VSCODE_DELAY);
    }
    #[cfg(windows)]
    let mut cmd = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    let _ = cmd.arg(url).spawn();
}

fn new_uuid() -> String {
    let mut b = [0_u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(kind_req: Value) -> Pending {
        let view = ask_view("r1", &kind_req);
        Pending {
            view,
            input: kind_req["input"].clone(),
            suggestions: kind_req["permission_suggestions"].clone(),
            tool_use_id: kind_req["tool_use_id"].as_str().map(str::to_owned),
        }
    }

    #[test]
    fn permission_asks_and_answers() {
        let p = pending(json!({
            "subtype": "can_use_tool", "tool_name": "Bash", "tool_use_id": "tu1",
            "input": {"command": "npm test", "description": "Run the tests"},
            "permission_suggestions": [{"type": "addRules", "behavior": "allow", "destination": "session",
                "rules": [{"toolName": "Bash", "ruleContent": "npm test:*"}]}]
        }));
        assert_eq!(p.view.kind, AskKind::Permission);
        assert_eq!(p.view.summary, "npm test");
        assert_eq!(p.view.detail.as_deref(), Some("Run the tests"));
        assert_eq!(
            p.view.always.as_deref(),
            Some("Bash(npm test:*) · trong phiên này")
        );
        let allow = tool_response(&p, &AskAnswer::Allow { always: false }).unwrap();
        assert_eq!(allow["behavior"], "allow");
        assert_eq!(allow["updatedInput"]["command"], "npm test");
        assert_eq!(allow["toolUseID"], "tu1");
        assert!(allow.get("updatedPermissions").is_none());
        let always = tool_response(&p, &AskAnswer::Allow { always: true }).unwrap();
        assert_eq!(
            always["updatedPermissions"][0]["rules"][0]["ruleContent"],
            "npm test:*"
        );
        let no = tool_response(&p, &AskAnswer::Deny { message: None }).unwrap();
        assert_eq!(
            (no["behavior"].clone(), no["interrupt"].clone()),
            (json!("deny"), json!(true))
        );
        let why = tool_response(
            &p,
            &AskAnswer::Deny {
                message: Some("dùng pnpm".into()),
            },
        )
        .unwrap();
        assert_eq!(
            (why["message"].clone(), why["interrupt"].clone()),
            (json!("dùng pnpm"), json!(false))
        );
        assert!(tool_response(&p, &AskAnswer::ApprovePlan { mode: None }).is_none());
    }

    #[test]
    fn questions_and_plans() {
        let q = pending(json!({
            "subtype": "can_use_tool", "tool_name": "AskUserQuestion",
            "input": {"questions": [{"question": "Màu nào?", "header": "Màu", "multiSelect": false,
                "options": [{"label": "Đỏ", "description": "nóng"}, {"label": "Xanh"}]}]}
        }));
        assert_eq!(q.view.kind, AskKind::Question);
        assert_eq!(q.view.questions[0].options[1].label, "Xanh");
        assert!(q.view.always.is_none());
        let mut answers = std::collections::BTreeMap::new();
        answers.insert("Màu nào?".to_owned(), "Xanh".to_owned());
        let r = tool_response(&q, &AskAnswer::Answers { answers }).unwrap();
        assert_eq!(r["updatedInput"]["answers"]["Màu nào?"], "Xanh");
        assert_eq!(r["updatedInput"]["questions"][0]["header"], "Màu");

        let plan = pending(json!({
            "subtype": "can_use_tool", "tool_name": "ExitPlanMode", "input": {"plan": "1. Sửa\n2. Test"}
        }));
        assert_eq!(plan.view.plan.as_deref(), Some("1. Sửa\n2. Test"));
        let ok = tool_response(
            &plan,
            &AskAnswer::ApprovePlan {
                mode: Some("auto".into()),
            },
        )
        .unwrap();
        assert_eq!(ok["behavior"], "allow");
        let again = tool_response(
            &plan,
            &AskAnswer::RejectPlan {
                feedback: Some("thêm test".into()),
            },
        )
        .unwrap();
        assert_eq!(again["behavior"], "deny");
        assert!(
            again["message"]
                .as_str()
                .unwrap()
                .ends_with("Feedback: thêm test")
        );
    }

    #[test]
    fn args_names_and_status() {
        let a = session_args("abc", false, Some("plan"), false);
        assert!(a.windows(2).any(|w| w == ["--session-id", "abc"]));
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "plan"]));
        assert!(
            a.windows(2)
                .any(|w| w == ["--setting-sources", "user,project,local"])
        );
        assert!(a.contains(&"--permission-prompt-tool=stdio".to_owned()));
        let r = session_args("abc", true, None, true);
        assert!(r.windows(2).any(|w| w == ["--resume", "abc"]));
        assert!(r.windows(2).any(|w| w == ["--setting-sources", "user"]));
        assert!(!r.contains(&"--permission-mode".to_owned()));
        assert_eq!(
            project_dir_name(r"C:\Users\me\.op-workflow-dev\session-probe"),
            "C--Users-me--op-workflow-dev-session-probe"
        );
        let (id, held) = parse_status(
            r#"{"pid":42,"sessionId":"s1","status":"waiting","entrypoint":"claude-vscode","kind":"interactive"}"#,
        )
        .unwrap();
        assert_eq!(
            (id.as_str(), held.pid, held.status.as_str()),
            ("s1", 42, "waiting")
        );
        assert_eq!(where_open(&held.entrypoint), "VS Code");
        assert_eq!(
            first_line("\n  Sửa lỗi đăng nhập \nchi tiết", 80),
            "Sửa lỗi đăng nhập"
        );
    }

    /// Follows one session the way the chat pane does: every patch replaces the items from its
    /// `from` on.
    struct Follow {
        rx: std::sync::mpsc::Receiver<SessionPatch>,
        items: Vec<opw_domain::view::ChatItem>,
        seen: Vec<String>,
    }

    impl Follow {
        /// Waits until `done(items, patch)`; on a timeout says what was seen.
        fn until(
            &mut self,
            what: &str,
            done: impl Fn(&[opw_domain::view::ChatItem], &SessionPatch) -> bool,
        ) -> SessionPatch {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(150) {
                let Ok(p) = self.rx.recv_timeout(Duration::from_millis(500)) else {
                    continue;
                };
                self.items.truncate(p.from as usize);
                self.items.extend(p.items.iter().cloned());
                let note = format!(
                    "{:?} live={} asks={} items={} err={:?}",
                    p.session.state,
                    p.session.live,
                    p.asks.len(),
                    self.items.len(),
                    p.session.error
                );
                if self.seen.last() != Some(&note) {
                    self.seen.push(note);
                }
                if done(&self.items, &p) {
                    return p;
                }
            }
            let texts: Vec<String> = self
                .items
                .iter()
                .map(|i| format!("{:?}: {}", i.kind, cut(&i.text, 80)))
                .collect();
            panic!(
                "timed out waiting for {what}
seen: {:#?}
items: {texts:#?}",
                self.seen
            );
        }
    }

    fn idle_with<'a>(
        marks: &'a [&'a str],
    ) -> impl Fn(&[opw_domain::view::ChatItem], &SessionPatch) -> bool + 'a {
        move |items, p| {
            p.session.state == SessionState::Idle
                && items
                    .iter()
                    .rev()
                    .take_while(|i| i.kind != opw_domain::view::ChatKind::User)
                    .any(|i| marks.iter().any(|m| i.text.contains(m)))
        }
    }

    /// The real CLI, end to end: start, a folder trust answer, a turn, a refused permission, the
    /// process ended and the session resumed. Run with a cheap model:
    /// `ANTHROPIC_MODEL=claude-haiku-5-5 cargo test -p opw-orchestrator live_session -- --ignored`
    #[test]
    #[ignore = "runs the real claude CLI (a few cents)"]
    fn live_session_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let sessions = Sessions::new(store, dir.path().join("hooks"));
        let (tx, rx) = std::sync::mpsc::channel();
        sessions.set_sink(Arc::new(move |p| {
            let _ = tx.send(p);
        }));
        let mut follow = Follow {
            rx,
            items: Vec::new(),
            seen: Vec::new(),
        };
        let repo_text = repo.to_string_lossy().into_owned();
        let id = sessions.create(None, &repo_text).unwrap().id;
        sessions.set_mode(&id, "default").unwrap();

        // a folder nobody trusted yet: asked first (answered "no", so nothing is recorded)
        let v = sessions
            .send(
                &id,
                "Reply with exactly PONG-WS and nothing else. Do not use tools.",
                false,
            )
            .unwrap();
        if v.state == SessionState::Waiting {
            sessions
                .answer(&id, TRUST_ASK, &AskAnswer::Trust { trust: false })
                .unwrap();
        }
        let p = follow.until("the first answer", idle_with(&["PONG-WS"]));
        assert!(p.session.live);
        assert!(p.session.model.is_some());

        // a permission request reaches the app; refused, the command does not run
        sessions
            .send(
                &id,
                "Use the Bash tool to run exactly: echo opw > made.txt   If it is refused, reply with only REFUSED.",
                false,
            )
            .unwrap();
        let ask = follow.until("a permission request", |_, p| {
            p.asks.iter().any(|a| a.kind == AskKind::Permission)
        });
        let ask_id = ask.asks[0].id.clone();
        sessions
            .answer(
                &id,
                &ask_id,
                &AskAnswer::Deny {
                    message: Some("Not now.".into()),
                },
            )
            .unwrap();
        follow.until("the turn after the refusal", |_, p| {
            p.session.state == SessionState::Idle && p.asks.is_empty()
        });
        assert!(!repo.join("made.txt").exists());

        // the process ends; the next message resumes the same session. A fresh session knows
        // neither word, so either one shows the conversation came back.
        sessions.stop(&id).unwrap();
        sessions
            .send(
                &id,
                "What exact word did you reply with to my very first message in this conversation? Reply with only that word.",
                false,
            )
            .unwrap();
        follow.until("the answer after resuming", idle_with(&["PONG-WS", "REFUSED"]));
        let chat = sessions.chat(&id).unwrap();
        assert!(
            chat.items
                .iter()
                .any(|i| i.kind == opw_domain::view::ChatKind::User)
        );
        assert!(chat.session.cost_usd > 0.0);
        sessions.shutdown(Duration::from_secs(10));
    }

    #[test]
    fn suggestions_in_words() {
        let s = json!([
            {"type": "addDirectories", "directories": ["D:/data"], "destination": "session"},
            {"type": "addRules", "rules": [{"toolName": "WebFetch"}], "destination": "localSettings"}
        ]);
        assert_eq!(
            describe_suggestions(&s).as_deref(),
            Some(
                "thư mục D:/data · trong phiên này; WebFetch · lưu vào .claude/settings.local.json của repo"
            )
        );
        assert_eq!(describe_suggestions(&json!([])), None);
    }
}
