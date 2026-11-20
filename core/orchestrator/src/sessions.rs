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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use opw_domain::view::{ApiError, QuotaSnapshot};
use opw_storage::{SessionRow, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::flow_mcp::{self, ActiveStep, SessionStep};
use crate::terminals::{Pane, TerminalKind, TerminalSink, statusline_settings};
use crate::workspace_tools::{self, FileLists};
use opw_domain::session::{ImageIn, RepoScript, SlashCommand};
use opw_domain::workflow::StepKind;

/// Sessions the app runs say so (`entrypoint` in the transcript and the status file). Without it
/// the CLI writes `sdk-cli`, and the VS Code extension leaves those out of its session list;
/// `cli` is turned back into `sdk-cli` when there is no terminal.
pub const ENTRYPOINT: &str = "op-workflow";
/// A session with nothing to do for this long gives its process back (the next message resumes).
const IDLE_CLOSE: Duration = Duration::from_secs(30 * 60);
/// How often the janitor looks.
const JANITOR_EVERY: Duration = Duration::from_secs(1);
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

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum TerminalAction {
    Ack,
    Detach,
    Close,
    Write(String),
    Resize { cols: u16, rows: u16 },
}

fn terminal_size((cols, rows): (u16, u16)) -> (u16, u16) {
    (cols.clamp(2, 500), rows.clamp(2, 300))
}

fn resize_pane(pane: &Pane, size: (u16, u16)) -> Result<(), ApiError> {
    let (cols, rows) = terminal_size(size);
    pane.pty
        .resize(cols, rows)
        .map_err(|e| err("terminal.resize", e.to_string()))
}

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
    total_cost: f64,
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
    terminal: Option<Pane>,
    shell: Option<(TerminalKind, Pane)>,
    statusline_at: Option<SystemTime>,
    context_percent: Option<f64>,
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
    held: Vec<Queued>,
    /// set_permission_mode request id → mode, to fall back from a refused `auto`.
    mode_requests: HashMap<String, String>,
    next_req: u64,
    active_step: ActiveStep,
    step_labels: HashMap<String, StepKind>,
    step_result: Option<opw_agents::claude::RunResult>,
    step_output: String,
    step_cost: f64,
    step_mode: Option<String>,
    step_model: Option<String>,
    mcp: Option<flow_mcp::Run>,
    /// The `/` commands the running CLI announced.
    commands: Vec<String>,
    /// Lines from the app in this chat (not sent to the AI), kept apart from the transcript.
    notes: Vec<AppNote>,
}

/// A line from the app in a session's chat: shown after the first `after` items of the
/// conversation (counting items dropped from the front), never sent to the AI.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppNote {
    after: usize,
    text: String,
}

/// kv kind of a task session's app notes (key = session id).
const NOTES_KV: &str = "session-notes";

/// A message waiting to be sent (until the folder trust question is answered).
struct Queued {
    text: String,
    uuid: Option<String>,
    images: Vec<ImageIn>,
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
            terminal: None,
            shell: None,
            statusline_at: None,
            context_percent: None,
            last_pid: None,
            live_mode: None,
            open_turns: 0,
            last_activity: Instant::now(),
            seq: 0,
            error: None,
            held: Vec::new(),
            mode_requests: HashMap::new(),
            next_req: 0,
            active_step: Arc::default(),
            step_labels: HashMap::new(),
            step_result: None,
            step_output: String::new(),
            step_cost: 0.0,
            step_mode: None,
            step_model: None,
            mcp: None,
            commands: Vec::new(),
            notes: Vec::new(),
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
        let ours = |h: &Held| {
            self.proc.as_ref().is_some_and(|p| p.pid == h.pid)
                || self.terminal.as_ref().and_then(|p| p.pty.pid()) == Some(h.pid)
                || self.last_pid == Some(h.pid)
        };
        let outside = held.filter(|h| !ours(h)).map(|h| where_open(&h.entrypoint));
        let terminal_status = self.terminal.as_ref().and_then(|pane| {
            let pid = pane.pty.pid()?;
            held.filter(|h| h.pid == pid).map(|h| h.status.as_str())
        });
        let state = if !self.asks.is_empty() || matches!(terminal_status, Some("waiting")) {
            SessionState::Waiting
        } else if (self.proc.is_some() && self.open_turns > 0) || matches!(terminal_status, Some("busy")) {
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
            terminal: self.terminal.is_some(),
            shell: self.shell.as_ref().map(|(kind, _)| kind.key().to_owned()),
            context_percent: self.context_percent,
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

    /// The chat items from the `from`-th item of the log on, with the app's notes in their
    /// places, and where they start in the list the UI holds.
    fn items_from(&self, from: usize) -> (usize, Vec<opw_domain::view::ChatItem>) {
        let dropped = self.log.dropped();
        let items = self.log.items();
        let from = from.min(items.len());
        let before = self.notes.iter().filter(|n| n.after < dropped + from).count();
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate().skip(from) {
            out.extend(self.notes_at(dropped + i));
            out.push(self.chat_item(item.clone()));
        }
        // notes after the last item, and those of items no longer held
        out.extend(
            self.notes
                .iter()
                .filter(|n| n.after >= dropped + items.len())
                .map(|n| note_item(&n.text)),
        );
        (from + before, out)
    }

    fn notes_at(&self, after: usize) -> impl Iterator<Item = opw_domain::view::ChatItem> + '_ {
        self.notes
            .iter()
            .filter(move |n| n.after == after)
            .map(|n| note_item(&n.text))
    }

    fn chat(&self, held: Option<&Held>) -> SessionChat {
        SessionChat {
            session: self.view(held),
            items: self.items_from(0).1,
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

    fn say(&mut self, text: &str, uuid: Option<&str>, images: &[ImageIn]) -> Result<(), ApiError> {
        if uuid.is_some() {
            if let Some(mode) = self.step_mode.take() {
                self.request_mode(&mode)?;
            }
            if let Some(model) = self.step_model.take() {
                self.control(json!({"subtype":"set_model","model":model}))?;
            }
        }
        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
        for image in images {
            content.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": image.media_type, "data": image.data },
            }));
        }
        let mut payload = json!({
            "type": "user",
            "message": { "role": "user", "content": content },
            "parent_tool_use_id": null,
            "session_id": "",
        });
        if let Some(uuid) = uuid {
            payload["uuid"] = json!(uuid);
        }
        self.write(&payload)?;
        if uuid.is_some() {
            self.log.apply(&payload);
        }
        self.open_turns += 1;
        self.last_activity = Instant::now();
        self.started = true;
        if self.row.title.is_empty() && !text.trim().is_empty() {
            self.row.title = first_line(text, TITLE_MAX);
        }
        Ok(())
    }

    fn chat_item(&self, i: opw_agents::transcript::Item) -> opw_domain::view::ChatItem {
        let step = i
            .message_id
            .as_ref()
            .and_then(|id| self.step_labels.get(id))
            .copied();
        let mut item = crate::flow::chat_item(i);
        item.step = step;
        item
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
        self.held.clear();
        self.mcp = None;
    }

    fn pane(&self, kind: TerminalKind) -> Option<&Pane> {
        match kind {
            TerminalKind::Claude => self.terminal.as_ref(),
            _ => self.shell.as_ref().filter(|(k, _)| *k == kind).map(|(_, p)| p),
        }
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

pub(crate) fn claude_home() -> Option<PathBuf> {
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

/// A line from the app as a chat item.
fn note_item(text: &str) -> opw_domain::view::ChatItem {
    opw_domain::view::ChatItem {
        kind: opw_domain::view::ChatKind::Note,
        step: None,
        text: text.to_owned(),
        tool: None,
        output: None,
        output_cut: 0,
        failed: false,
        denied: None,
    }
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

pub(crate) fn hide_window(cmd: &mut Command) {
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
    create_task: Mutex<()>,
    files: FileLists,
    /// The `/` commands the last CLI announced (for a session not running now).
    last_commands: Mutex<Vec<String>>,
    quitting: AtomicBool,
    quota: Mutex<Option<QuotaSink>>,
}

type QuotaSink = Arc<dyn Fn(QuotaSnapshot) + Send + Sync>;

/// The sessions the Workspace shows. Cheap to clone.
#[derive(Clone)]
pub struct Sessions {
    inner: Arc<Inner>,
}

pub(crate) struct StepReply {
    pub result: opw_agents::claude::RunResult,
    pub summary: String,
    pub manual: bool,
}

pub(crate) struct StepMessage<'a> {
    pub step: StepKind,
    pub prompt: &'a str,
    pub mode: Option<&'a str>,
    pub model: Option<&'a str>,
    pub wait_for_report: bool,
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
                create_task: Mutex::new(()),
                files: FileLists::default(),
                last_commands: Mutex::new(Vec::new()),
                quitting: AtomicBool::new(false),
                quota: Mutex::new(None),
            }),
        }
    }

    /// Where pushed changes go (the shell turns them into a Tauri event).
    pub fn set_sink(&self, sink: PatchSink) {
        *lock(&self.inner.sink) = Some(sink);
    }

    pub fn set_quota_sink(&self, sink: impl Fn(QuotaSnapshot) + Send + Sync + 'static) {
        *lock(&self.inner.quota) = Some(Arc::new(sink));
    }

    fn observe_quota(&self, snap: QuotaSnapshot) {
        if let Some(sink) = lock(&self.inner.quota).clone() {
            sink(snap);
        }
    }

    fn ensure_open(&self) -> Result<(), ApiError> {
        if self.inner.quitting.load(Ordering::Acquire) {
            return Err(err("session.quitting", "App đang thoát"));
        }
        Ok(())
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
            if e.terminal.is_some() {
                self.observe_statusline(&id, &mut e);
            }
            if e.terminal.as_mut().is_some_and(|p| p.pty.exit_code().is_some()) {
                let _ = e.terminal.take().expect("checked").close();
                e.mcp = None;
                e.refresh();
                self.emit(&id, &mut e);
            }
            if e.shell.as_mut().is_some_and(|(_, p)| p.pty.exit_code().is_some()) {
                let _ = e.shell.take().expect("checked").1.close();
                self.emit(&id, &mut e);
            }
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
            } else if e.last_activity.elapsed() > IDLE_CLOSE && lock(&e.active_step).is_none() {
                if let Some(proc) = e.proc.take() {
                    close_proc(proc, EXIT_WAIT);
                }
                e.mcp = None;
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
        let (from, items) = e.items_from(from);
        let patch = SessionPatch {
            id: id.to_owned(),
            seq: e.seq,
            from: u32::try_from(from).unwrap_or(u32::MAX),
            items,
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
        if let Some(wp_id) = entry.row.wp_id {
            let read = |kind: &str| self.store().kv_get(wp_id, kind, &id).ok().flatten();
            entry.step_labels = read("session-step-labels")
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            entry.notes = read(NOTES_KV)
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
        }
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

    /// A line from the app at the end of a task session's chat (not sent to the AI): the brief
    /// changed, the task moved to another repo. Kept with the task, so reading the transcript
    /// again keeps it.
    pub fn note(&self, id: &str, text: &str) -> Result<(), ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        let wp_id = e
            .row
            .wp_id
            .ok_or_else(|| err("session.task", "Phiên này không thuộc task nào"))?;
        let after = e.log.dropped() + e.log.items().len();
        e.notes.push(AppNote {
            after,
            text: text.trim().to_owned(),
        });
        let json = serde_json::to_string(&e.notes).map_err(|x| err("storage", x.to_string()))?;
        self.store()
            .kv_set(wp_id, NOTES_KV, id, &json, &now())
            .map_err(|x| err("storage", x.to_string()))?;
        self.emit(id, &mut e);
        Ok(())
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

    pub fn ensure_task(&self, wp_id: u64, repo: &str) -> Result<SessionView, ApiError> {
        let _serial = lock(&self.inner.create_task);
        if let Some(id) = self.task_session(wp_id) {
            let arc = self.entry(&id)?;
            let e = lock(&arc);
            if opw_config::same_path(&e.row.repo, repo) {
                return Ok(e.view(None));
            }
        }
        self.create(Some(wp_id), repo)
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

    fn observe_statusline(&self, id: &str, e: &mut Entry) {
        let Some(dir) = self.inner.hooks_dir.parent() else {
            return;
        };
        let path = dir.join("events").join(format!("{id}.jsonl"));
        let Some(at) = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()) else {
            return;
        };
        if e.statusline_at == Some(at) {
            return;
        }
        let line = (|| {
            let mut file = std::fs::File::open(&path).ok()?;
            let len = file.metadata().ok()?.len();
            file.seek(SeekFrom::Start(len.saturating_sub(8192))).ok()?;
            let mut tail = Vec::new();
            file.take(8192).read_to_end(&mut tail).ok()?;
            tail.rsplit(|&b| b == b'\n')
                .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
                .find(|p| p["kind"] == "statusline" && p["session_id"] == id)
        })();
        let Some(line) = line else { return };
        e.statusline_at = Some(at);
        if let Some(snap) = opw_quota::from_claude_statusline(&line, &now()) {
            self.observe_quota(snap);
        }
        let percent = line["context_window"]["used_percentage"].as_f64();
        if percent != e.context_percent {
            e.context_percent = percent;
            self.emit(id, e);
        }
    }

    pub fn terminal_open(
        &self,
        id: &str,
        kind: TerminalKind,
        view: String,
        size: (u16, u16),
        sink: TerminalSink,
    ) -> Result<(), ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        self.ensure_open()?;
        if let Some(pane) = e.pane(kind) {
            resize_pane(pane, size)?;
            pane.attach(view, sink);
            return Ok(());
        }
        let cwd = opw_config::expand_home(&e.row.repo);
        if !cwd.is_dir() {
            return Err(err("session.repo", "Không còn thư mục của phiên"));
        }
        let mut env = opw_vcs::guard_env(&self.inner.hooks_dir);
        let mut mcp = None;
        let (program, args) = if kind == TerminalKind::Claude {
            if e.open_turns > 0 || !e.asks.is_empty() || lock(&e.active_step).is_some() {
                return Err(err(
                    "session.busy",
                    "Kết thúc lượt hiện tại trong chat trước khi mở terminal",
                ));
            }
            if let Some(h) = status_files().get(id).filter(|h| e.last_pid != Some(h.pid)) {
                if e.proc.as_ref().is_none_or(|p| p.pid != h.pid) {
                    return Err(err(
                        "session.outside",
                        format!("Phiên đang mở ở {}", where_open(&h.entrypoint)),
                    ));
                }
            }
            let exe =
                cli::find_claude().ok_or_else(|| err("claude.missing", "Không tìm thấy claude trên PATH"))?;
            if let Some(proc) = e.proc.take() {
                close_proc(proc, EXIT_WAIT);
            }
            e.clear_turn();
            e.refresh();
            let resume = e.transcript.as_ref().is_some_and(|p| p.is_file());
            let mut args = vec![
                if resume { "--resume" } else { "--session-id" }.into(),
                id.to_owned(),
                "--setting-sources".into(),
                "user,project,local".into(),
                "--disallowedTools".into(),
                DISALLOWED_GIT.into(),
            ];
            if let Some(mode) = e.row.mode.as_ref() {
                args.extend(["--permission-mode".into(), mode.clone()]);
            }
            env.push(("CLAUDE_CODE_ENTRYPOINT".into(), ENTRYPOINT.into()));
            if let Ok(me) = std::env::current_exe() {
                env.push((
                    "OPW_STATUSLINE_EXE".into(),
                    me.to_string_lossy().replace('\\', "/"),
                ));
                env.push(("OPW_STATUSLINE_REPO".into(), cwd.to_string_lossy().into_owned()));
                if let Some(root) = self.inner.hooks_dir.parent() {
                    env.push((
                        opw_config::ENV_DATA_DIR.into(),
                        root.to_string_lossy().into_owned(),
                    ));
                }
                if let Some(shell) = crate::terminals::git_bash() {
                    env.push((
                        "OPW_STATUSLINE_SHELL".into(),
                        shell.to_string_lossy().into_owned(),
                    ));
                }
                args.extend(["--settings".into(), statusline_settings()]);
            }
            mcp = self.prepare_mcp(&e, &cwd, &mut args);
            if let Some(server) = &mcp {
                env.push((flow_mcp::TOKEN_ENV.into(), server.token().into()));
            }
            (exe, args)
        } else {
            if e.shell.is_some() {
                return Err(err("terminal.shell", "Đóng shell đang mở để đổi sang shell khác"));
            }
            kind.shell()
                .ok_or_else(|| err("terminal.missing", "Không tìm thấy shell đã chọn"))?
        };
        let (cols, rows) = terminal_size(size);
        let pane = Pane::spawn(&opw_pty::SpawnSpec {
            program: &program,
            args: &args,
            cwd: &cwd,
            remove_env: &stripped_env(),
            set_env: &env,
            cols,
            rows,
        });
        let pane = match pane {
            Ok(pane) => pane,
            Err(failure) => {
                self.emit(id, &mut e);
                return Err(err("terminal.spawn", failure.to_string()));
            }
        };
        pane.attach(view, sink);
        if kind == TerminalKind::Claude {
            e.last_pid = pane.pty.pid();
            e.statusline_at = None;
            e.terminal = Some(pane);
            e.mcp = mcp;
        } else {
            e.shell = Some((kind, pane));
        }
        self.emit(id, &mut e);
        Ok(())
    }

    pub fn terminal_control(
        &self,
        id: &str,
        kind: TerminalKind,
        view: &str,
        action: TerminalAction,
    ) -> Result<(), ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        let Some(pane) = e.pane(kind).filter(|p| p.owns(view)) else {
            return Ok(());
        };
        match action {
            TerminalAction::Ack => pane.ack(view),
            TerminalAction::Detach => pane.detach(view),
            TerminalAction::Write(data) => {
                if data.len() > 256 * 1024 {
                    return Err(err("terminal.input", "Nội dung dán quá dài"));
                }
                pane.pty
                    .write(data.as_bytes())
                    .map_err(|e| err("terminal.write", e.to_string()))?;
            }
            TerminalAction::Resize { cols, rows } => resize_pane(pane, (cols, rows))?,
            TerminalAction::Close => {
                let pane = match kind {
                    TerminalKind::Claude => e.terminal.take(),
                    _ => e.shell.take().map(|(_, p)| p),
                };
                let result = pane.expect("owned pane").close();
                if kind == TerminalKind::Claude {
                    e.mcp = None;
                    e.refresh();
                }
                self.emit(id, &mut e);
                result.map_err(|e| err("terminal.close", e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Send a message: started (or resumed) first when nothing runs. `force` sends even though
    /// the session looks open somewhere else (a stale status file after a crash).
    pub fn send(
        &self,
        id: &str,
        text: &str,
        force: bool,
        images: &[ImageIn],
    ) -> Result<SessionView, ApiError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(err("session.empty", "Tin nhắn trống"));
        }
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        if e.open_turns > 0 && lock(&e.active_step).is_some() {
            return Err(err(
                "session.busy",
                "Chờ tin nhắn của bước trả lời xong trước khi gửi tiếp",
            ));
        }
        check_images(images)?;
        self.send_locked(id, &mut e, text, force, None, images)
    }

    fn send_locked(
        &self,
        id: &str,
        e: &mut Entry,
        text: &str,
        force: bool,
        uuid: Option<&str>,
        images: &[ImageIn],
    ) -> Result<SessionView, ApiError> {
        self.ensure_open()?;
        if e.terminal.is_some() {
            return Err(err(
                "session.terminal",
                "Đóng Terminal của phiên để tiếp tục trong chat",
            ));
        }
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
                e.held.push(Queued {
                    text: text.to_owned(),
                    uuid: uuid.map(str::to_owned),
                    images: images.to_vec(),
                });
                return Ok(e.view(None));
            }
            e.refresh();
            match self.trust_run(&e.row.repo) {
                Some(run) => self.spawn(id, e, &run)?,
                None => {
                    e.held.push(Queued {
                        text: text.to_owned(),
                        uuid: uuid.map(str::to_owned),
                        images: images.to_vec(),
                    });
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
                    self.emit(id, e);
                    return Ok(e.view(None));
                }
            }
        }
        e.say(text, uuid, images)?;
        e.error = None;
        self.save(e);
        self.emit(id, e);
        Ok(e.view(None))
    }

    fn prepare_mcp(&self, e: &Entry, repo: &Path, args: &mut Vec<String>) -> Option<flow_mcp::Run> {
        e.row.wp_id?;
        let server = flow_mcp::Run::workspace(repo, e.active_step.clone()).ok()?;
        server.configure_workspace(args);
        Some(server)
    }

    pub(crate) fn flow_ready(&self, id: &str) -> Result<(), ApiError> {
        let arc = self.entry(id)?;
        let e = lock(&arc);
        self.flow_ready_entry(id, &e)
    }

    fn flow_ready_entry(&self, id: &str, e: &Entry) -> Result<(), ApiError> {
        if e.terminal.is_some() {
            return Err(err(
                "session.terminal",
                "Đóng Terminal của phiên để chạy bước trong chat",
            ));
        }
        if e.view(status_files().get(id)).outside.is_some() {
            return Err(err(
                "session.outside",
                "Đóng phiên ở ứng dụng ngoài trước khi chạy bước",
            ));
        }
        if e.open_turns > 0 || !e.asks.is_empty() || lock(&e.active_step).is_some() {
            return Err(err(
                "session.busy",
                "Chờ lượt hiện tại của phiên kết thúc trước khi chạy bước",
            ));
        }
        Ok(())
    }

    pub(crate) fn run_flow_step(
        &self,
        id: &str,
        message: StepMessage<'_>,
        stop: &AtomicBool,
        timeout: Duration,
    ) -> Result<StepReply, ApiError> {
        let StepMessage {
            step,
            prompt,
            mode,
            model,
            wait_for_report,
        } = message;
        let arc = self.entry(id)?;
        let uuid = new_uuid();
        {
            let mut e = lock(&arc);
            self.flow_ready_entry(id, &e)?;
            self.ensure_open()?;
            *lock(&e.active_step) = Some(SessionStep {
                id: uuid.clone(),
                name: crate::flow::step_key(step).to_owned(),
                ..SessionStep::default()
            });
            e.step_result = None;
            e.step_output.clear();
            e.step_cost = 0.0;
            e.step_mode = mode.map(str::to_owned);
            e.step_model = model.map(str::to_owned);
            e.step_labels.insert(uuid.clone(), step);
            if let Some(wp_id) = e.row.wp_id {
                if let Ok(labels) = serde_json::to_string(&e.step_labels) {
                    let _ = self
                        .store()
                        .kv_set(wp_id, "session-step-labels", id, &labels, &now());
                }
            }
            if let Err(error) = self.send_locked(id, &mut e, prompt, false, Some(&uuid), &[]) {
                lock(&e.active_step).take();
                return Err(error);
            }
        }
        let started = Instant::now();
        loop {
            {
                let mut e = lock(&arc);
                let stopped = stop.load(Ordering::Acquire) || self.inner.quitting.load(Ordering::Acquire);
                if stopped || started.elapsed() >= timeout {
                    if e.open_turns > 0 {
                        let _ = e.control(json!({"subtype":"interrupt"}));
                        if let Some(p) = e.proc.take() {
                            close_proc(p, EXIT_WAIT);
                        }
                    }
                    if e.proc.is_none() {
                        e.clear_turn();
                    }
                    lock(&e.active_step).take();
                    self.emit(id, &mut e);
                    return Err(if stopped {
                        err("step.stopped", "Bạn đã dừng bước này")
                    } else {
                        err("step.timeout", "Hết thời gian chờ bước")
                    });
                }
                if e.proc.is_none() && e.asks.is_empty() {
                    lock(&e.active_step).take();
                    return Err(err(
                        "session.stopped",
                        e.error.as_deref().unwrap_or("Phiên đã đóng"),
                    ));
                }
                if e.open_turns == 0
                    && e.step_result.as_ref().is_some_and(|r| r.is_error)
                    && e.step_output.is_empty()
                {
                    let result = e.step_result.take().expect("checked");
                    lock(&e.active_step).take();
                    return Err(err(
                        "session.result",
                        if result.text.is_empty() {
                            "Claude không hoàn tất lượt này. Xem chat rồi thử lại.".into()
                        } else {
                            result.text
                        },
                    ));
                }
                let done = {
                    let mut active = lock(&e.active_step);
                    let valid = active.as_ref().is_some_and(|s| {
                        s.manual || s.summary.is_some() || (!wait_for_report && e.step_result.is_some())
                    });
                    if valid && e.open_turns == 0 && e.asks.is_empty() {
                        active.take()
                    } else {
                        None
                    }
                };
                if let Some(done) = done {
                    let mut result = e.step_result.take().unwrap_or_default();
                    result.session_id = id.to_owned();
                    result.cost_usd = e.step_cost;
                    if !e.step_output.trim().is_empty() {
                        result.text = std::mem::take(&mut e.step_output);
                    }
                    return Ok(StepReply {
                        result,
                        manual: done.manual,
                        summary: done.summary.unwrap_or_else(|| {
                            if done.manual {
                                "Bạn đánh dấu xong".into()
                            } else {
                                "Đã trả lời trong phiên".into()
                            }
                        }),
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    pub(crate) fn complete_flow_step(&self, id: &str, step: StepKind) -> Result<(), ApiError> {
        let arc = self.entry(id)?;
        let e = lock(&arc);
        if e.open_turns > 0 || !e.asks.is_empty() {
            return Err(err("session.busy", "Chờ lượt hiện tại kết thúc rồi bấm Xong"));
        }
        let mut active = lock(&e.active_step);
        let current = active
            .as_mut()
            .filter(|s| s.name == crate::flow::step_key(step))
            .ok_or_else(|| err("step.missing", "Bước này chưa gửi vào phiên hoặc đã xong"))?;
        current.manual = true;
        Ok(())
    }

    /// The session's repo folder and task.
    pub fn place(&self, id: &str) -> Result<(PathBuf, Option<u64>), ApiError> {
        let arc = self.entry(id)?;
        let e = lock(&arc);
        Ok((opw_config::expand_home(&e.row.repo), e.row.wp_id))
    }

    /// Repo files whose path holds `query`, for `@` in the chat.
    pub fn files(&self, id: &str, query: &str) -> Result<Vec<String>, ApiError> {
        let (repo, _) = self.place(id)?;
        Ok(self.inner.files.find(&repo, query, 30))
    }

    /// The `/` commands of this session.
    pub fn commands(&self, id: &str) -> Result<Vec<SlashCommand>, ApiError> {
        let arc = self.entry(id)?;
        let (repo, mut known) = {
            let e = lock(&arc);
            (opw_config::expand_home(&e.row.repo), e.commands.clone())
        };
        if known.is_empty() {
            known = lock(&self.inner.last_commands).clone();
        }
        Ok(workspace_tools::slash_commands(
            &repo,
            &known,
            claude_home().as_deref(),
        ))
    }

    /// The repo's package.json scripts.
    pub fn scripts(&self, id: &str) -> Result<Vec<RepoScript>, ApiError> {
        let (repo, _) = self.place(id)?;
        Ok(workspace_tools::scripts(&repo))
    }

    /// Title and text of a notification for a session that waits for the user.
    pub fn wait_note(&self, id: &str) -> Option<(String, String)> {
        let arc = self.entry(id).ok()?;
        let e = lock(&arc);
        let ask = e.asks.first()?;
        let title = if e.row.title.is_empty() {
            "Phiên chờ bạn".to_owned()
        } else {
            format!("Chờ bạn · {}", e.row.title)
        };
        let body = match ask.view.kind {
            AskKind::Permission => format!(
                "Claude xin chạy {}: {}",
                ask.view.tool,
                first_line(&ask.view.summary, 120)
            ),
            AskKind::Question => ask
                .view
                .questions
                .first()
                .map_or_else(|| "Claude hỏi bạn".to_owned(), |q| first_line(&q.question, 160)),
            AskKind::Plan => "Claude chờ bạn duyệt plan".to_owned(),
            AskKind::Trust => "Repo chưa được tin cậy: trả lời trong chat để bắt đầu".to_owned(),
        };
        Some((title, body))
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
        let mut args = session_args(id, resume, e.row.mode.as_deref(), *trust == TrustRun::UserOnly);
        let mcp = self.prepare_mcp(e, &repo, &mut args);
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
        if let Some(server) = &mcp {
            cmd.env(flow_mcp::TOKEN_ENV, server.token());
        }
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
            total_cost: 0.0,
        });
        e.mcp = mcp;
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
            "rate_limit_event" => {
                if let Some(snap) = opw_quota::from_claude_rate_limit_event(v, &now()) {
                    self.observe_quota(snap);
                }
            }
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
                    let commands: Vec<String> = v["slash_commands"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c.as_str().map(str::to_owned))
                        .collect();
                    if !commands.is_empty() {
                        lock(&self.inner.last_commands).clone_from(&commands);
                        e.commands = commands;
                    }
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
                if lock(&e.active_step).is_some()
                    && v["parent_tool_use_id"].is_null()
                    && e.step_output.len() < 128 * 1024
                {
                    for block in v["message"]["content"].as_array().into_iter().flatten() {
                        if block["type"] == "text" {
                            if let Some(text) = block["text"].as_str() {
                                e.step_output.push_str(text);
                                e.step_output.push_str("\n\n");
                            }
                        }
                    }
                }
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
                if let Some(total) = v["total_cost_usd"].as_f64() {
                    let delta = e.proc.as_mut().map_or(0.0, |proc| {
                        let delta = (total - proc.total_cost).max(0.0);
                        proc.total_cost = total;
                        delta
                    });
                    e.row.cost_usd += delta;
                    if lock(&e.active_step).is_some() {
                        e.step_cost += delta;
                    }
                }
                if lock(&e.active_step).is_some() {
                    let result = opw_agents::claude::parse_line(&v.to_string())
                        .ok()
                        .and_then(|events| {
                            events.into_iter().find_map(|event| match event {
                                opw_agents::claude::ClaudeEvent::Result(result) => Some(result),
                                _ => None,
                            })
                        });
                    if let Some(result) = result {
                        e.step_result = Some(result);
                    }
                }
                e.log.apply(v);
                e.open_turns = e.open_turns.saturating_sub(1);
                e.partial.clear();
                if e.open_turns == 0 {
                    e.asks.clear();
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
            for q in std::mem::take(&mut e.held) {
                e.say(&q.text, q.uuid.as_deref(), &q.images)?;
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
        if e.terminal.is_some() {
            return Err(err(
                "session.terminal",
                "Đổi chế độ ngay trong Terminal của phiên",
            ));
        }
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
            close_proc(proc, EXIT_WAIT);
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
        if e.terminal.is_some() {
            return Err(err(
                "session.terminal",
                "Đóng Terminal của phiên trước khi mở VS Code",
            ));
        }
        if !e.started {
            return Err(err("session.empty", "Phiên chưa có tin nào để mở trong VS Code"));
        }
        if let Some(proc) = e.proc.take() {
            close_proc(proc, EXIT_WAIT);
        }
        e.clear_turn();
        self.emit(id, &mut e);
        let repo = opw_config::expand_home(&e.row.repo);
        let url = format!("vscode://anthropic.claude-code/open?session={id}");
        drop(e);
        std::thread::spawn(move || {
            open_in_vscode(&repo, &url);
        });
        Ok(())
    }

    /// Take a session off the list (its transcript stays; the CLI can still resume it).
    pub fn hide(&self, id: &str) -> Result<(), ApiError> {
        let arc = self.entry(id)?;
        let mut e = lock(&arc);
        if e.terminal.is_some() || e.shell.is_some() {
            return Err(err("session.terminal", "Đóng terminal trước khi ẩn phiên"));
        }
        if let Some(proc) = e.proc.take() {
            close_proc(proc, EXIT_WAIT);
        }
        e.clear_turn();
        self.store()
            .hide_session(id, &now())
            .map_err(|e| err("storage", e.to_string()))?;
        drop(e);
        lock(&self.inner.entries).remove(id);
        Ok(())
    }

    /// Sessions with a turn running now (the close dialog counts them).
    pub fn running(&self) -> usize {
        self.all_entries()
            .iter()
            .filter(|(_, arc)| {
                let e = lock(arc);
                (e.proc.is_some() && e.open_turns > 0) || e.terminal.is_some() || e.shell.is_some()
            })
            .count()
    }

    /// End every process (the app quits); waits at most `wait` before killing what is left.
    pub fn shutdown(&self, wait: Duration) {
        self.inner.quitting.store(true, Ordering::Release);
        let handles: Vec<_> = self
            .all_entries()
            .into_iter()
            .map(|(_, arc)| {
                std::thread::spawn(move || {
                    let mut e = lock(&arc);
                    e.clear_turn();
                    if let Some(p) = e.terminal.take() {
                        let _ = p.close();
                    }
                    if let Some((_, p)) = e.shell.take() {
                        let _ = p.close();
                    }
                    if let Some(p) = e.proc.take() {
                        close_proc(p, wait);
                    }
                })
            })
            .collect();
        for h in handles {
            let _ = h.join();
        }
    }
}

/// `code <repo>` (the folder's window opens or comes to the front), then the session link.
/// `\\?\`, in front of every path `std::fs::canonicalize` gives on Windows.
const VERBATIM: &str = "\\\\?\\";

/// Open `path` (relative to `repo`, which it must stay inside) in VS Code: the repo's window,
/// at `line`.
pub(crate) fn open_file_in_vscode(repo: &Path, path: &str, line: Option<u32>) -> Result<(), ApiError> {
    let root = std::fs::canonicalize(repo).map_err(|_| err("file.repo", "Không còn thư mục repo"))?;
    let file = std::fs::canonicalize(root.join(path))
        .map_err(|_| err("file.missing", format!("Không còn file {path}")))?;
    if !file.starts_with(&root) {
        return Err(err("file.outside", "File nằm ngoài repo"));
    }
    let code = cli::find_in_path(
        &["code.cmd", "code"],
        &std::env::var_os("PATH").unwrap_or_default(),
    )
    .ok_or_else(|| err("vscode.missing", "Không tìm thấy lệnh code (VS Code) trên PATH"))?;
    // canonical paths on Windows start with the verbatim prefix, which VS Code does not take
    let plain = |p: &Path| p.to_string_lossy().trim_start_matches(VERBATIM).to_owned();
    let target = match line {
        Some(n) => format!("{}:{n}", plain(&file)),
        None => plain(&file),
    };
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(code).arg(plain(&root)).arg("-g").arg(target);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| err("vscode.open", e.to_string()))
}

/// Characters `cmd /C` would read as its own (the file names go through it to `code`).
fn cmd_unsafe(path: &str) -> bool {
    path.chars()
        .any(|c| matches!(c, '&' | '|' | '<' | '>' | '^' | '%' | '"'))
}

/// Show a changed file next to its version at `rev` (the fork point the diff was made from) in
/// VS Code's diff view, in the repo's window. The old version is written to a scratch folder; a
/// file the change added is compared with an empty one.
pub(crate) fn compare_in_vscode(
    repo: &Path,
    rev: &str,
    path: &str,
    old_path: Option<&str>,
) -> Result<(), ApiError> {
    if !(4..=40).contains(&rev.len()) || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err("diff.rev", "Không rõ commit gốc để so sánh"));
    }
    let before = old_path.unwrap_or(path);
    let outside = |p: &str| p.split(['/', '\\']).any(|part| part == "..") || Path::new(p).is_absolute();
    if outside(path) || outside(before) {
        return Err(err("file.outside", "File nằm ngoài repo"));
    }
    if cmd_unsafe(path) || cmd_unsafe(before) {
        return Err(err(
            "file.name",
            "Tên file có ký tự app không chuyển cho VS Code được. Mở file bằng nút Mở trong VS Code.",
        ));
    }
    let root = std::fs::canonicalize(repo).map_err(|_| err("file.repo", "Không còn thư mục repo"))?;
    let file = std::fs::canonicalize(root.join(path))
        .map_err(|_| err("file.missing", format!("Không còn file {path}")))?;
    if !file.starts_with(&root) {
        return Err(err("file.outside", "File nằm ngoài repo"));
    }
    let old = opw_vcs::Git::new(&root)
        .run(&["show", &format!("{rev}:{before}")])
        .unwrap_or_default();
    let left = std::env::temp_dir()
        .join("op-workflow-compare")
        .join(rev)
        .join(path);
    left.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&left, old))
        .map_err(|e| err("diff.write", format!("Không ghi được bản cũ của {path}: {e}")))?;
    let code = cli::find_in_path(
        &["code.cmd", "code"],
        &std::env::var_os("PATH").unwrap_or_default(),
    )
    .ok_or_else(|| err("vscode.missing", "Không tìm thấy lệnh code (VS Code) trên PATH"))?;
    let plain = |p: &Path| p.to_string_lossy().trim_start_matches(VERBATIM).to_owned();
    let mut cmd = Command::new("cmd");
    cmd.arg("/C")
        .arg(code)
        .arg(plain(&root))
        .arg("--diff")
        .arg(plain(&left))
        .arg(plain(&file));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| err("vscode.open", e.to_string()))
}

/// Open the repo's folder in VS Code (its window if one is open), leaving the session here.
pub(crate) fn open_repo_in_vscode(repo: &Path) -> Result<(), ApiError> {
    let root = std::fs::canonicalize(repo).map_err(|_| err("file.repo", "Không còn thư mục repo"))?;
    let plain = root.to_string_lossy().trim_start_matches(VERBATIM).to_owned();
    if cmd_unsafe(&plain) {
        return Err(err(
            "file.name",
            "Đường dẫn repo có ký tự app không chuyển cho VS Code được",
        ));
    }
    let code = cli::find_in_path(
        &["code.cmd", "code"],
        &std::env::var_os("PATH").unwrap_or_default(),
    )
    .ok_or_else(|| err("vscode.missing", "Không tìm thấy lệnh code (VS Code) trên PATH"))?;
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(code).arg(plain);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| err("vscode.open", e.to_string()))
}

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

/// What the API takes: a few images of the usual kinds, each under 5 MB.
fn check_images(images: &[ImageIn]) -> Result<(), ApiError> {
    const KINDS: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
    const MAX_IMAGES: usize = 8;
    const MAX_BYTES: usize = 5 * 1024 * 1024;
    if images.len() > MAX_IMAGES {
        return Err(err(
            "session.images",
            format!("Gửi tối đa {MAX_IMAGES} ảnh mỗi tin"),
        ));
    }
    for image in images {
        if !KINDS.contains(&image.media_type.as_str()) {
            return Err(err("session.images", "Chỉ gửi được ảnh PNG, JPEG, GIF hoặc WebP"));
        }
        if image.data.len() / 4 * 3 > MAX_BYTES {
            return Err(err("session.images", "Mỗi ảnh tối đa 5 MB"));
        }
    }
    Ok(())
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
    fn app_notes_sit_between_the_messages_they_followed() {
        let row = SessionRow {
            id: "s-1".into(),
            wp_id: Some(1),
            repo: "C:/r".into(),
            kind: "main".into(),
            title: String::new(),
            mode: None,
            model: None,
            cost_usd: 0.0,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let mut e = Entry::new(row, None, None);
        let say = |e: &mut Entry, text: &str| {
            e.log
                .apply(&json!({"type":"assistant","message":{"content":[{"type":"text","text":text}]}}));
        };
        say(&mut e, "một");
        e.notes.push(AppNote {
            after: 1,
            text: "Brief đổi".into(),
        });
        say(&mut e, "hai");
        let texts = |items: &[opw_domain::view::ChatItem]| {
            items
                .iter()
                .map(|i| format!("{:?}:{}", i.kind, i.text))
                .collect::<Vec<_>>()
        };
        let (from, items) = e.items_from(0);
        assert_eq!(from, 0);
        assert_eq!(texts(&items), ["Text:một", "Note:Brief đổi", "Text:hai"]);
        // a patch from the second message carries the note that sits before it: the list the UI
        // rebuilds (first `from` items kept) is the same
        let (from, items) = e.items_from(1);
        assert_eq!(
            (from, texts(&items)),
            (1, vec!["Note:Brief đổi".to_owned(), "Text:hai".to_owned()])
        );
        // a note added after the last message comes with a patch from the end
        e.notes.push(AppNote {
            after: 2,
            text: "Repo đổi".into(),
        });
        let (from, items) = e.items_from(2);
        assert_eq!((from, texts(&items)), (3, vec!["Note:Repo đổi".to_owned()]));
    }

    #[test]
    fn compare_refuses_what_it_cannot_pass_on_safely() {
        // every refusal comes before VS Code or git is touched
        let repo = Path::new(".");
        let code = |rev: &str, path: &str, old: Option<&str>| {
            compare_in_vscode(repo, rev, path, old)
                .map_err(|e| e.code)
                .unwrap_err()
        };
        assert_eq!(code("main", "a.ts", None), "diff.rev");
        assert_eq!(code("abc", "a.ts", None), "diff.rev");
        assert_eq!(code("abc1234;", "a.ts", None), "diff.rev");
        assert_eq!(code("abc1234", "../secret.txt", None), "file.outside");
        assert_eq!(code("abc1234", "src/a.ts", Some("..\\x.ts")), "file.outside");
        assert_eq!(code("abc1234", "C:/Windows/win.ini", None), "file.outside");
        assert_eq!(code("abc1234", "a&calc.ts", None), "file.name");
        assert_eq!(code("abc1234", "a.ts", Some("b%PATH%.ts")), "file.name");
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
                &[],
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
                &[],
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
                &[],
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
