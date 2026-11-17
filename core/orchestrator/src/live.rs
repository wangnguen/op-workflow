//! Live data (Phase 1, read-only): the OpenProject inbox and AI quota, refreshed on background
//! threads so IPC calls only read a cache and never wait on the network.
//!
//! - Tasks: `HttpTracker::inbox` every `tracker.pollMin` minutes, or right away on `refresh`.
//!   Comments of a task are read again only when its `updatedAt` moved (PLAN.md §8).
//! - Codex quota: one long-lived `codex app-server` (JSON-RPC over stdio), polled every
//!   `quota.codexPollSec` seconds; restarted if it dies.
//! - Claude quota, newest reading wins:
//!   - Claude Code's own cache (`~/.claude.json` `cachedUsageUtilization`, the numbers of its
//!     usage panel), read again whenever the file changes; free;
//!   - each workflow step's `rate_limit_event`; free;
//!   - a minimal `claude -p` probe (~735 Haiku tokens) in an empty folder, reading its
//!     `rate_limit_event` (PLAN.md §7), only when the newest reading is older than
//!     `quota.claudeProbeMin` minutes, or `quota.claudeActiveProbeSec` seconds while Claude Code
//!     is being used outside the app (a transcript under `~/.claude/projects` was just written).
//!
//! Nothing here writes to OpenProject.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use opw_agents::cli;
use opw_agents::env::{Account, is_stripped};
use opw_domain::view::PriorityView;
use opw_domain::view::{LinkStatus, Provider, ProviderLink, QuotaSnapshot};
use opw_tracker::{Comment, HttpTracker, Me, TrackerApi, WorkPackage};
use serde_json::{Value, json};

const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const PROBE_TIMEOUT: Duration = Duration::from_secs(90);
const RESTART_BACKOFF: Duration = Duration::from_secs(60);
/// How often a healthy app-server connection checks for a "Kết nối lại" request.
const WAKE_CHECK: Duration = Duration::from_secs(1);
/// How often the Claude quota thread looks at Claude Code's cache file (one `stat`).
const CLAUDE_TICK: Duration = Duration::from_secs(3);
/// How often it lists Claude Code's transcripts to see whether Claude Code is in use.
const ACTIVITY_SCAN: Duration = Duration::from_secs(15);

/// Cached live state. The token stays inside the process and is never serialized.
#[derive(Clone, Default)]
pub struct LiveState {
    /// OpenProject URL (empty until entered in Settings).
    pub base_url: String,
    pub token: Option<String>,
    pub nx_user: Option<String>,
    /// OpenProject id of the token owner: comments with this author are the user's own (no
    /// "cần xem", no bell). `None` until `users/me` answers.
    pub nx_user_id: Option<u64>,
    /// `None` until the first successful fetch.
    pub tasks: Option<Vec<WorkPackage>>,
    pub last_sync: Option<String>,
    pub tasks_error: Option<String>,
    /// OpenProject status name → `#rrggbb`, refreshed with the inbox.
    pub status_colors: Vec<(String, String)>,
    /// Priority name → `#rrggbb`, refreshed with the inbox (when the server gives colours).
    pub priority_colors: Vec<(String, String)>,
    /// Priority names from the lowest to the highest, as OpenProject orders them.
    pub priority_order: Vec<String>,
    /// Comments of the inbox tasks (work package id → comments, oldest first).
    pub comments: HashMap<u64, Vec<Comment>>,
    pub claude: Option<QuotaSnapshot>,
    pub codex: Option<QuotaSnapshot>,
    /// `None` until the quota threads start.
    pub claude_link: Option<ProviderLink>,
    pub codex_link: Option<ProviderLink>,
    /// How often Claude is probed (Settings → Hạn mức changes it without a restart).
    pub claude_probe: ClaudeProbe,
}

impl LiveState {
    pub fn status_color(&self, status: &str) -> Option<String> {
        self.status_colors
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(status))
            .map(|(_, c)| c.clone())
    }

    /// A task's priority as OpenProject shows it (`None` when it has no name).
    pub fn priority(&self, name: &str) -> Option<PriorityView> {
        let name = name.trim();
        let at = self
            .priority_order
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name));
        (!name.is_empty()).then(|| PriorityView {
            name: name.to_owned(),
            color: self
                .priority_colors
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, c)| c.clone()),
            rank: at.and_then(|i| u8::try_from(i + 1).ok()),
            ranks: at.and_then(|_| u8::try_from(self.priority_order.len()).ok()),
        })
    }

    pub fn links(&self) -> Vec<ProviderLink> {
        self.claude_link
            .iter()
            .chain(self.codex_link.iter())
            .cloned()
            .collect()
    }

    /// Problems only, one line each (Settings shows them under Hạn mức).
    pub fn quota_error(&self) -> Option<String> {
        let errors: Vec<&str> = [&self.claude_link, &self.codex_link]
            .into_iter()
            .flatten()
            .filter(|l| {
                matches!(
                    l.status,
                    LinkStatus::Error | LinkStatus::Missing | LinkStatus::SignedOut
                )
            })
            .filter_map(|l| l.message.as_deref())
            .collect();
        (!errors.is_empty()).then(|| errors.join("\n"))
    }

    /// Keep the newest Claude reading: probes, the steps' own events and Claude Code's cache
    /// arrive out of order. `false` when `snap` is not newer than the one kept.
    pub fn put_claude(&mut self, snap: QuotaSnapshot) -> bool {
        let newer = self.claude.as_ref().is_none_or(|old| {
            opw_quota::seconds_between(&old.observed_at, &snap.observed_at).is_none_or(|s| s > 0.0)
        });
        if newer {
            self.claude = Some(snap);
        }
        newer
    }

    fn link_mut(&mut self, provider: Provider) -> &mut Option<ProviderLink> {
        match provider {
            Provider::Claude => &mut self.claude_link,
            Provider::Codex => &mut self.codex_link,
        }
    }
}

impl std::fmt::Debug for LiveState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveState")
            .field("token_set", &self.token.is_some())
            .field("tasks", &self.tasks.as_ref().map(Vec::len))
            .field("tasks_error", &self.tasks_error)
            .finish_non_exhaustive()
    }
}

/// How often the Claude probe runs, from `quota.claudeProbeMin` (Claude Code not in use) and
/// `quota.claudeActiveProbeSec` (in use outside the app; 0 = off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeProbe {
    pub idle: Duration,
    pub active: Option<Duration>,
}

impl ClaudeProbe {
    pub fn from_config(q: &opw_config::QuotaConfig) -> Self {
        Self {
            idle: Duration::from_secs(u64::from(q.claude_probe_min.max(1)) * 60),
            active: (q.claude_active_probe_sec > 0).then(|| {
                Duration::from_secs(u64::from(
                    q.claude_active_probe_sec.max(opw_config::MIN_ACTIVE_PROBE_SEC),
                ))
            }),
        }
    }
}

impl Default for ClaudeProbe {
    fn default() -> Self {
        Self::from_config(&opw_config::QuotaConfig::default())
    }
}

#[derive(Debug, Clone)]
pub struct LiveSettings {
    pub poll: Duration,
    pub codex_poll: Duration,
    pub claude_probe: ClaudeProbe,
    pub claude_probe_model: String,
    /// Empty folder for the Claude probe, so no CLAUDE.md is loaded.
    pub probe_dir: PathBuf,
}

/// Told each task's comments as the inbox poll reads them (the app keeps them for the next start).
pub type CommentsSink = Box<CommentsFn>;
pub type CommentsFn = dyn Fn(u64, &[Comment]) + Send;

#[derive(Debug)]
pub struct Live {
    state: Arc<Mutex<LiveState>>,
    wake_tasks: Sender<()>,
    waker: Waker,
    pub settings: LiveSettings,
}

/// Wakes the quota threads for "Kết nối lại"; cheap to clone into a waiting thread.
#[derive(Debug, Clone)]
struct Waker {
    state: Arc<Mutex<LiveState>>,
    claude: Sender<()>,
    codex: Sender<()>,
}

impl Waker {
    fn reconnect(&self, provider: Option<Provider>) {
        for p in [Provider::Claude, Provider::Codex] {
            if provider.is_some_and(|x| x != p) {
                continue;
            }
            set_link(&self.state, p, LinkStatus::Connecting, None);
            let wake = match p {
                Provider::Claude => &self.claude,
                Provider::Codex => &self.codex,
            };
            let _ = wake.send(());
        }
    }
}

fn now() -> String {
    opw_quota::now_rfc3339()
}

impl Live {
    /// Start the background threads. `token` is the saved OpenProject token, if any.
    /// `saved`: comments kept from the last run, shown until the first poll reads them again;
    /// `on_comments`: told every fresh read.
    pub fn start(
        settings: LiveSettings,
        base_url: String,
        token: Option<String>,
        saved: HashMap<u64, Vec<Comment>>,
        on_comments: Option<CommentsSink>,
    ) -> Self {
        let state = Arc::new(Mutex::new(LiveState {
            base_url,
            token,
            comments: saved,
            claude_link: Some(link(Provider::Claude, LinkStatus::Connecting, None)),
            codex_link: Some(link(Provider::Codex, LinkStatus::Connecting, None)),
            claude_probe: settings.claude_probe,
            ..LiveState::default()
        }));
        let (wake_tasks, wake_rx) = mpsc::channel();
        spawn_named("opw-tasks", {
            let (state, s) = (state.clone(), settings.clone());
            move || tasks_loop(&state, &s, &wake_rx, on_comments.as_deref())
        });
        let (wake_codex, codex_rx) = mpsc::channel();
        spawn_named("opw-codex-quota", {
            let (state, every) = (state.clone(), settings.codex_poll);
            move || codex_loop(&state, every, &codex_rx)
        });
        let (wake_claude, claude_rx) = mpsc::channel();
        spawn_named("opw-claude-quota", {
            let (state, s) = (state.clone(), settings.clone());
            move || claude_loop(&state, &s, &claude_rx)
        });
        let waker = Waker {
            state: state.clone(),
            claude: wake_claude,
            codex: wake_codex,
        };
        Self {
            state,
            wake_tasks,
            waker,
            settings,
        }
    }

    pub fn snapshot(&self) -> LiveState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Use a token that the caller has already checked with `HttpTracker::me`.
    pub fn set_token(&self, base_url: String, token: String, user: Me) {
        if let Ok(mut s) = self.state.lock() {
            s.base_url = base_url;
            s.token = Some(token);
            s.nx_user = Some(user.name);
            s.nx_user_id = Some(user.id);
            s.tasks = None;
            s.tasks_error = None;
            s.comments.clear();
        }
        self.refresh();
    }

    pub fn clear_token(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.token = None;
            s.nx_user = None;
            s.nx_user_id = None;
            s.tasks = None;
            s.tasks_error = None;
            s.last_sync = None;
            s.comments.clear();
        }
    }

    /// Comments the Comment tab just read, so badges follow without waiting for the poll.
    pub fn put_comments(&self, wp_id: u64, comments: Vec<Comment>) {
        update(&self.state, |s| {
            s.comments.insert(wp_id, comments);
        });
    }

    /// Quota read from a workflow step's own `rate_limit_event` (every headless Claude run
    /// reports it for free, PLAN.md §9).
    pub fn observe_claude(&self, snap: QuotaSnapshot) {
        put_claude(&self.state, snap);
    }

    /// Fetch tasks now instead of waiting for the next poll.
    pub fn refresh(&self) {
        let _ = self.wake_tasks.send(());
    }

    /// Settings → Hạn mức changed how often Claude is probed; the Claude thread reads it on its
    /// next tick.
    pub fn set_claude_probe(&self, probe: ClaudeProbe) {
        update(&self.state, |s| s.claude_probe = probe);
    }

    /// "Kết nối lại": restart a dead Codex app-server or read it again now; run the Claude
    /// probe now. `None` means both.
    pub fn reconnect(&self, provider: Option<Provider>) {
        self.waker.reconnect(provider);
    }

    /// Open the CLI's own sign-in (`claude auth login`, `codex login`) in a console window;
    /// reconnect once that window closes.
    pub fn open_login(&self, provider: Provider) -> Result<(), String> {
        let (exe, args) = match provider {
            Provider::Claude => (cli::find_claude(), "auth login"),
            Provider::Codex => (cli::find_codex(), "login"),
        };
        let exe = exe.ok_or_else(|| missing_message(provider))?;
        let mut child = login_console(&exe, args).spawn().map_err(|e| e.to_string())?;
        let waker = self.waker.clone();
        spawn_named("opw-login-wait", move || {
            let _ = child.wait();
            waker.reconnect(Some(provider));
        });
        Ok(())
    }
}

fn missing_message(provider: Provider) -> String {
    match provider {
        Provider::Claude => "Không thấy lệnh claude trên PATH. Cài Claude Code rồi bấm Kết nối lại.",
        Provider::Codex => "Không thấy lệnh codex trên PATH. Cài Codex CLI rồi bấm Kết nối lại.",
    }
    .to_owned()
}

fn signed_out_message(provider: Provider) -> String {
    let name = match provider {
        Provider::Claude => "Claude",
        Provider::Codex => "Codex",
    };
    format!("{name} chưa đăng nhập hoặc phiên đăng nhập đã hết hạn. Bấm Đăng nhập để mở cửa sổ đăng nhập.")
}

/// Drop the API-key variables, so the CLI uses the subscription like the extensions do.
fn strip_env(cmd: &mut Command) -> &mut Command {
    for (k, _) in std::env::vars() {
        if is_stripped(&k, Account::Subscription) {
            cmd.env_remove(k);
        }
    }
    cmd
}

/// A visible console running `<exe> <args>`, kept open with `pause` so the result stays readable.
#[cfg(windows)]
fn login_console(exe: &Path, args: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let mut cmd = Command::new("cmd.exe");
    // cmd /c strips the outer quotes and runs: "<exe>" <args> & echo. & pause
    cmd.raw_arg(format!("/c \"\"{}\" {args} & echo. & pause\"", exe.display()))
        .creation_flags(CREATE_NEW_CONSOLE);
    strip_env(&mut cmd);
    cmd
}

#[cfg(not(windows))]
fn login_console(exe: &Path, args: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args.split(' '));
    strip_env(&mut cmd);
    cmd
}

/// `Some(false)` when `claude auth status` says nobody is signed in; `None` when unsure.
fn claude_signed_in(exe: &Path) -> Option<bool> {
    let out = hidden(
        Command::new(exe)
            .args(["auth", "status", "--json"])
            .stdin(Stdio::null())
            .stderr(Stdio::null()),
    )
    .output()
    .ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    v.get("loggedIn").and_then(Value::as_bool)
}

/// `Some(false)` when `codex login status` says nobody is signed in; `None` when unsure.
fn codex_signed_in(exe: &Path) -> Option<bool> {
    let out = hidden(Command::new(exe).args(["login", "status"]).stdin(Stdio::null()))
        .output()
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    if text.contains("not logged in") {
        Some(false)
    } else if out.status.success() && text.contains("logged in") {
        Some(true)
    } else {
        None
    }
}

fn link(provider: Provider, status: LinkStatus, message: Option<String>) -> ProviderLink {
    ProviderLink {
        provider,
        status,
        message,
        since: now(),
    }
}

/// Keep `since` when nothing changed, so the UI can say how long a problem has lasted.
fn set_link(state: &Mutex<LiveState>, provider: Provider, status: LinkStatus, message: Option<String>) {
    update(state, |s| {
        let slot = s.link_mut(provider);
        if slot
            .as_ref()
            .is_some_and(|l| l.status == status && l.message == message)
        {
            return;
        }
        *slot = Some(link(provider, status, message));
    });
}

/// Wait for a reconnect request or the timeout; `false` when the app is shutting down.
fn wait_wake(wake: &Receiver<()>, timeout: Duration) -> bool {
    match wake.recv_timeout(timeout) {
        Ok(()) => {
            while wake.try_recv().is_ok() {}
            true
        }
        Err(RecvTimeoutError::Timeout) => true,
        Err(RecvTimeoutError::Disconnected) => false,
    }
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.to_owned()).spawn(f);
}

fn update(state: &Mutex<LiveState>, f: impl FnOnce(&mut LiveState)) {
    if let Ok(mut s) = state.lock() {
        f(&mut s);
    }
}

// ---------- tasks ----------

fn tasks_loop(
    state: &Mutex<LiveState>,
    settings: &LiveSettings,
    wake: &Receiver<()>,
    on_comments: Option<&CommentsFn>,
) {
    // `updatedAt` of each task when its comments were last read
    let mut read_at: HashMap<u64, String> = HashMap::new();
    loop {
        let (base_url, token, need_user) = state
            .lock()
            .map(|s| (s.base_url.clone(), s.token.clone(), s.nx_user_id.is_none()))
            .unwrap_or_default();
        if let Some(token) = token.filter(|_| !base_url.is_empty()) {
            let tracker = HttpTracker::new(&base_url, &token);
            let user = need_user.then(|| tracker.me().ok()).flatten();
            let result = tracker.inbox();
            let colors = tracker.status_colors().ok().filter(|c| !c.is_empty());
            let priorities = tracker.priorities().ok();
            let cached: Vec<u64> = state
                .lock()
                .map(|s| s.comments.keys().copied().collect())
                .unwrap_or_default();
            let fresh = match &result {
                Ok(tasks) => fetch_changed_comments(&tracker, tasks, &cached, &mut read_at),
                Err(_) => HashMap::new(),
            };
            if let Some(keep) = on_comments {
                for (wp_id, list) in &fresh {
                    keep(*wp_id, list);
                }
            }
            update(state, |s| {
                // the token may have been cleared or replaced while we were fetching
                if s.token.as_deref() != Some(token.as_str()) {
                    return;
                }
                if let Some(me) = user {
                    s.nx_user = Some(me.name);
                    s.nx_user_id = Some(me.id);
                }
                if let Some(colors) = colors {
                    s.status_colors = colors;
                }
                if let Some(p) = priorities {
                    if !p.colors.is_empty() {
                        s.priority_colors = p.colors;
                    }
                    if !p.order.is_empty() {
                        s.priority_order = p.order;
                    }
                }
                match result {
                    Ok(tasks) => {
                        s.comments.retain(|id, _| tasks.iter().any(|t| t.id == *id));
                        s.comments.extend(fresh);
                        s.tasks = Some(tasks);
                        s.last_sync = Some(now());
                        s.tasks_error = None;
                    }
                    Err(e) => s.tasks_error = Some(e.to_string()),
                }
            });
        }
        match wake.recv_timeout(settings.poll) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Comments of the tasks whose `updatedAt` moved since the last read (all of them the first
/// time). A failed read is tried again on the next poll.
fn fetch_changed_comments(
    tracker: &HttpTracker,
    tasks: &[WorkPackage],
    cached: &[u64],
    read_at: &mut HashMap<u64, String>,
) -> HashMap<u64, Vec<Comment>> {
    read_at.retain(|id, _| tasks.iter().any(|t| t.id == *id));
    let mut out = HashMap::new();
    for wp in tasks {
        let same = read_at.get(&wp.id) == Some(&wp.updated_at) && !wp.updated_at.is_empty();
        if same && cached.contains(&wp.id) {
            continue;
        }
        if let Ok(comments) = tracker.comments(wp.id) {
            read_at.insert(wp.id, wp.updated_at.clone());
            out.insert(wp.id, comments);
        }
    }
    out
}

// ---------- child processes ----------

pub(crate) fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    strip_env(cmd)
}

/// Lines from a child's stdout, read on their own thread so callers can time out.
pub(crate) fn line_reader(child: &mut Child) -> Option<Receiver<String>> {
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    spawn_named("opw-child-stdout", move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
    Some(rx)
}

// ---------- codex quota ----------

fn codex_loop(state: &Mutex<LiveState>, every: Duration, wake: &Receiver<()>) {
    loop {
        match cli::find_codex() {
            Some(exe) => match run_app_server(&exe, state, every, wake) {
                Ok(()) => return,
                Err(_) if codex_signed_in(&exe) == Some(false) => set_link(
                    state,
                    Provider::Codex,
                    LinkStatus::SignedOut,
                    Some(signed_out_message(Provider::Codex)),
                ),
                Err(e) => set_link(
                    state,
                    Provider::Codex,
                    LinkStatus::Error,
                    Some(format!(
                        "codex app-server lỗi: {e}. App tự thử lại sau 1 phút, hoặc bấm Kết nối lại."
                    )),
                ),
            },
            None => set_link(
                state,
                Provider::Codex,
                LinkStatus::Missing,
                Some(missing_message(Provider::Codex)),
            ),
        }
        if !wait_wake(wake, RESTART_BACKOFF) {
            return;
        }
    }
}

fn rpc_send(stdin: &mut impl Write, msg: &Value) -> std::io::Result<()> {
    writeln!(stdin, "{msg}")?;
    stdin.flush()
}

/// Wait for the response with `id`; `account/rateLimits/updated` notifications seen meanwhile
/// also update the cache.
fn rpc_wait(rx: &Receiver<String>, id: u64, state: &Mutex<LiveState>) -> Result<Value, String> {
    let deadline = Instant::now() + RPC_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = rx.recv_timeout(left).map_err(|e| match e {
            RecvTimeoutError::Timeout => "no answer".to_owned(),
            RecvTimeoutError::Disconnected => "exited".to_owned(),
        })?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("method").and_then(Value::as_str) == Some("account/rateLimits/updated") {
            store_codex(state, v.get("params").unwrap_or(&Value::Null));
            continue;
        }
        if v.get("id").and_then(Value::as_u64) == Some(id) {
            return match v.get("error") {
                Some(err) => Err(err.to_string()),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
        }
    }
}

/// `false` when the answer holds no rate limits (e.g. signed out).
fn store_codex(state: &Mutex<LiveState>, result: &Value) -> bool {
    let Some(snap) = opw_quota::from_codex_app_server(result, &now()) else {
        return false;
    };
    update(state, |s| s.codex = Some(snap));
    set_link(state, Provider::Codex, LinkStatus::Ok, None);
    true
}

/// `Ok(())` only when the app is shutting down; any other end is an error to retry.
fn run_app_server(
    exe: &Path,
    state: &Mutex<LiveState>,
    every: Duration,
    wake: &Receiver<()>,
) -> Result<(), String> {
    let mut child = hidden(
        Command::new(exe)
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .spawn()
    .map_err(|e| e.to_string())?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let rx = line_reader(&mut child).ok_or("no stdout")?;
        let init = json!({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "op-workflow", "title": null, "version": env!("CARGO_PKG_VERSION")},
            "capabilities": null}});
        rpc_send(&mut stdin, &init).map_err(|e| e.to_string())?;
        rpc_wait(&rx, 1, state)?;
        rpc_send(&mut stdin, &json!({"method": "initialized"})).map_err(|e| e.to_string())?;
        let mut id = 2u64;
        loop {
            rpc_send(
                &mut stdin,
                &json!({"id": id, "method": "account/rateLimits/read"}),
            )
            .map_err(|e| e.to_string())?;
            let result = rpc_wait(&rx, id, state)?;
            if !store_codex(state, &result) && id == 2 {
                return Err("không đọc được số hạn mức".to_owned());
            }
            id += 1;
            // Keep reading notifications until the next poll or a "Kết nối lại" request.
            let next = Instant::now() + every;
            while let Some(left) = next.checked_duration_since(Instant::now()) {
                match wake.try_recv() {
                    Ok(()) => {
                        while wake.try_recv().is_ok() {}
                        break;
                    }
                    Err(TryRecvError::Disconnected) => return Ok(()),
                    Err(TryRecvError::Empty) => {}
                }
                match rx.recv_timeout(left.min(WAKE_CHECK)) {
                    Ok(line) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&line) {
                            if v.get("method").and_then(Value::as_str) == Some("account/rateLimits/updated") {
                                store_codex(state, v.get("params").unwrap_or(&Value::Null));
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return Err("exited".to_owned()),
                }
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

// ---------- claude quota ----------

/// Take a Claude reading if it is newer than the one shown; a new one means the source works.
fn put_claude(state: &Mutex<LiveState>, snap: QuotaSnapshot) {
    let mut newer = false;
    update(state, |s| newer = s.put_claude(snap));
    if newer {
        set_link(state, Provider::Claude, LinkStatus::Ok, None);
    }
}

/// Where Claude Code keeps its files: `$CLAUDE_CONFIG_DIR`, or `~/.claude` with the config file
/// beside it (`~/.claude.json`).
struct ClaudeHome {
    config: PathBuf,
    projects: PathBuf,
}

fn claude_home() -> Option<ClaudeHome> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        return Some(ClaudeHome {
            config: dir.join(".claude.json"),
            projects: dir.join("projects"),
        });
    }
    let home = dirs::home_dir()?;
    Some(ClaudeHome {
        config: home.join(".claude.json"),
        projects: home.join(".claude").join("projects"),
    })
}

/// Claude Code's own last usage reading, read again only when its config file changed. A file
/// caught half-written is tried again on the next tick.
fn read_claude_cache(state: &Mutex<LiveState>, path: &Path, seen: &mut Option<SystemTime>) {
    let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) else {
        return;
    };
    if *seen == Some(modified) {
        return;
    }
    let Some(config) = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return;
    };
    *seen = Some(modified);
    if let Some(snap) = opw_quota::from_claude_code_cache(&config) {
        put_claude(state, snap);
    }
}

/// Newest write to a Claude Code transcript (`projects/<folder>/<session>.jsonl`; one line per
/// turn), so a recent time means Claude Code is spending quota right now. The app's own probe
/// keeps no transcript, so it never counts.
fn last_claude_write(projects: &Path) -> Option<SystemTime> {
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|d| std::fs::read_dir(d.path()).ok())
        .flat_map(|dir| dir.flatten())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

/// Whether a probe is due: the newest reading (`None` = none yet) and the last probe are both
/// older than the interval, which is `active` while Claude Code is in use elsewhere. After a
/// failed probe only the normal interval counts, so a broken CLI is not retried every minute.
fn probe_due(
    reading_age: Option<Duration>,
    since_probe: Option<Duration>,
    last_ok: bool,
    every: Duration,
    active: Option<Duration>,
) -> bool {
    let reading_every = active.map_or(every, |a| a.min(every));
    let probe_every = if last_ok { reading_every } else { every };
    reading_age.is_none_or(|a| a >= reading_every) && since_probe.is_none_or(|p| p >= probe_every)
}

fn claude_age(state: &Mutex<LiveState>) -> Option<Duration> {
    let at = state
        .lock()
        .ok()
        .and_then(|s| s.claude.as_ref().map(|c| c.observed_at.clone()))?;
    let secs = opw_quota::seconds_between(&at, &now())?;
    Some(Duration::from_secs_f64(secs.max(0.0)))
}

fn claude_loop(state: &Mutex<LiveState>, settings: &LiveSettings, wake: &Receiver<()>) {
    let home = claude_home();
    let mut seen = None;
    // (when the transcripts were last listed, the newest write found)
    let mut scanned: Option<(Instant, Option<SystemTime>)> = None;
    let mut last_probe: Option<(Instant, bool)> = None;
    let mut asked = false;
    loop {
        if let Some(h) = &home {
            read_claude_cache(state, &h.config, &mut seen);
        }
        // Claude Code wrote a transcript within two active intervals: one more probe after its
        // last turn catches the final number
        let every = state.lock().map(|s| s.claude_probe).unwrap_or_default();
        let active = every.active.filter(|a| {
            let Some(h) = &home else { return false };
            if scanned.is_none_or(|(at, _)| at.elapsed() >= ACTIVITY_SCAN) {
                scanned = Some((Instant::now(), last_claude_write(&h.projects)));
            }
            scanned
                .and_then(|(_, w)| w)
                .and_then(|w| w.elapsed().ok())
                .is_some_and(|ago| ago < *a * 2)
        });
        let due = probe_due(
            claude_age(state),
            last_probe.map(|(at, _)| at.elapsed()),
            last_probe.is_none_or(|(_, ok)| ok),
            every.idle,
            active,
        );
        if asked || due {
            let ok = probe_round(state, settings, every.idle);
            last_probe = Some((Instant::now(), ok));
        }
        asked = match wake.recv_timeout(CLAUDE_TICK) {
            Ok(()) => {
                while wake.try_recv().is_ok() {}
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
    }
}

/// One probe and the link status it leaves; `true` when it brought a reading.
fn probe_round(state: &Mutex<LiveState>, settings: &LiveSettings, idle: Duration) -> bool {
    let mins = idle.as_secs().div_ceil(60);
    match cli::find_claude() {
        Some(exe) => match probe_claude(&exe, settings) {
            Ok(Some(snap)) => {
                put_claude(state, snap);
                set_link(state, Provider::Claude, LinkStatus::Ok, None);
                true
            }
            Ok(_) | Err(_) if claude_signed_in(&exe) == Some(false) => {
                set_link(
                    state,
                    Provider::Claude,
                    LinkStatus::SignedOut,
                    Some(signed_out_message(Provider::Claude)),
                );
                false
            }
            Ok(None) => {
                set_link(
                    state,
                    Provider::Claude,
                    LinkStatus::Error,
                    Some(format!(
                        "Probe Claude chạy xong nhưng không có số hạn mức (rate_limit_event). Kiểm tra claude đã đăng nhập chưa; app thử lại sau {mins} phút."
                    )),
                );
                false
            }
            Err(e) => {
                set_link(
                    state,
                    Provider::Claude,
                    LinkStatus::Error,
                    Some(format!(
                        "Probe Claude lỗi: {e}. App thử lại sau {mins} phút, hoặc bấm Kết nối lại."
                    )),
                );
                false
            }
        },
        None => {
            set_link(
                state,
                Provider::Claude,
                LinkStatus::Missing,
                Some(missing_message(Provider::Claude)),
            );
            false
        }
    }
}

fn probe_claude(exe: &Path, settings: &LiveSettings) -> Result<Option<QuotaSnapshot>, String> {
    std::fs::create_dir_all(&settings.probe_dir).map_err(|e| e.to_string())?;
    let mut child = hidden(
        Command::new(exe)
            .args(opw_agents::claude::quota_probe_args(&settings.claude_probe_model))
            .current_dir(&settings.probe_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .spawn()
    .map_err(|e| e.to_string())?;
    let rx = line_reader(&mut child).ok_or("no stdout")?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut latest = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if v.get("type").and_then(Value::as_str) == Some("rate_limit_event") {
                        latest = opw_quota::from_claude_rate_limit_event(&v, &now()).or(latest);
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
        }
    }
    let _ = child.wait();
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_error_lists_only_problems() {
        let mut s = LiveState::default();
        assert!(s.links().is_empty() && s.quota_error().is_none());
        s.claude_link = Some(link(Provider::Claude, LinkStatus::Ok, None));
        s.codex_link = Some(link(
            Provider::Codex,
            LinkStatus::SignedOut,
            Some(signed_out_message(Provider::Codex)),
        ));
        assert_eq!(s.links().len(), 2);
        let err = s.quota_error().unwrap();
        assert!(err.starts_with("Codex chưa đăng nhập"), "{err}");
        // one problem, one line
        assert!(!err.contains('\n'));
    }

    #[test]
    fn priority_is_the_openproject_name_and_colour() {
        let s = LiveState {
            priority_colors: vec![("High".into(), "#ff922b".into())],
            ..LiveState::default()
        };
        let high = s.priority("high").unwrap();
        assert_eq!(
            (high.name.as_str(), high.color.as_deref()),
            ("high", Some("#ff922b"))
        );
        assert_eq!(s.priority("Normal").unwrap().color, None);
        assert!(s.priority(" ").is_none());
        // no order read yet: no rank
        assert_eq!((high.rank, high.ranks), (None, None));
        let s = LiveState {
            priority_order: vec!["Low".into(), "Normal".into(), "High".into(), "Immediate".into()],
            ..s
        };
        let high = s.priority("High").unwrap();
        assert_eq!((high.rank, high.ranks), (Some(3), Some(4)));
        assert_eq!(s.priority("Odd").unwrap().rank, None);
    }

    fn claude_at(observed_at: &str, five_hour: f64) -> QuotaSnapshot {
        QuotaSnapshot {
            provider: Provider::Claude,
            windows: vec![opw_domain::view::QuotaWindow {
                kind: opw_domain::view::QuotaWindowKind::FiveHour,
                used_pct: five_hour,
                resets_at: None,
            }],
            source: "test".into(),
            observed_at: observed_at.into(),
            plan: None,
        }
    }

    #[test]
    fn newest_claude_reading_wins() {
        let mut s = LiveState::default();
        assert!(s.put_claude(claude_at("2026-11-17T03:40:00Z", 29.0)));
        // Claude Code's cache, fetched earlier, arrives later: kept out
        assert!(!s.put_claude(claude_at("2026-11-17T03:34:42Z", 27.0)));
        assert!(!s.put_claude(claude_at("2026-11-17T03:40:00Z", 27.0)));
        assert!(s.put_claude(claude_at("2026-11-17T03:40:00.25Z", 30.0)));
        assert!((s.claude.unwrap().windows[0].used_pct - 30.0).abs() < 1e-9);
    }

    #[test]
    fn probe_runs_only_when_the_reading_is_old() {
        let m = |n: u64| Duration::from_secs(n * 60);
        let (every, active) = (m(15), Some(m(1)));
        // nothing yet: probe
        assert!(probe_due(None, None, true, every, None));
        // a fresh reading (Claude Code's cache, a step): no probe
        assert!(!probe_due(Some(m(3)), None, true, every, None));
        // Claude Code busy elsewhere: one minute is old enough
        assert!(probe_due(Some(m(3)), Some(m(2)), true, every, active));
        assert!(!probe_due(
            Some(Duration::from_secs(20)),
            Some(m(2)),
            true,
            every,
            active
        ));
        // the last probe was just now (it found nothing newer): wait
        assert!(!probe_due(
            Some(m(3)),
            Some(Duration::from_secs(10)),
            true,
            every,
            active
        ));
        // after a failed probe, retry on the normal cycle only
        assert!(!probe_due(Some(m(3)), Some(m(2)), false, every, active));
        assert!(probe_due(Some(m(20)), Some(m(15)), false, every, active));
        // idle: the normal cycle
        assert!(!probe_due(Some(m(10)), Some(m(10)), true, every, None));
        assert!(probe_due(Some(m(16)), Some(m(16)), true, every, None));
    }

    #[test]
    #[ignore = "reads this machine's Claude Code files"]
    fn live_claude_code_files() {
        let home = claude_home().expect("home folder");
        let state = Mutex::new(LiveState::default());
        read_claude_cache(&state, &home.config, &mut None);
        let snap = state
            .lock()
            .unwrap()
            .claude
            .clone()
            .expect("cachedUsageUtilization");
        let wrote = last_claude_write(&home.projects).expect("a transcript");
        println!(
            "{} {:?}; last transcript write {:?} ago",
            snap.observed_at,
            snap.windows,
            wrote.elapsed().unwrap_or_default()
        );
    }

    #[test]
    fn reads_claude_code_cache_when_the_file_changes() {
        let dir = std::env::temp_dir().join(format!("opw-cc-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".claude.json");
        let state = Mutex::new(LiveState::default());
        let mut seen = None;
        // half-written: nothing, and read again next time
        std::fs::write(&path, b"{\"oauthAccount\":").unwrap();
        read_claude_cache(&state, &path, &mut seen);
        assert!(seen.is_none() && state.lock().unwrap().claude.is_none());
        let config = json!({
            "oauthAccount": {"accountUuid": "a"},
            "cachedUsageUtilization": {"fetchedAtMs": 1_794_886_482_855_u64, "accountUuid": "a",
                "utilization": {"five_hour": {"utilization": 27}, "seven_day": {"utilization": 43}}}
        });
        std::fs::write(&path, config.to_string()).unwrap();
        read_claude_cache(&state, &path, &mut seen);
        let s = state.lock().unwrap().clone();
        assert_eq!(s.claude.unwrap().source, opw_quota::SOURCE_CLAUDE_CODE);
        assert_eq!(s.claude_link.unwrap().status, LinkStatus::Ok);
        assert!(seen.is_some());
        // a transcript next to it counts as Claude Code being in use
        let projects = dir.join("projects");
        assert!(last_claude_write(&projects).is_none());
        std::fs::create_dir_all(projects.join("c--repo")).unwrap();
        std::fs::write(projects.join("c--repo").join("s.jsonl"), b"{}\n").unwrap();
        std::fs::write(projects.join("c--repo").join("notes.txt"), b"x").unwrap();
        let wrote = last_claude_write(&projects).unwrap();
        assert!(wrote.elapsed().unwrap_or_default() < Duration::from_secs(60));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_link_keeps_since_when_unchanged() {
        let state = Mutex::new(LiveState::default());
        set_link(&state, Provider::Claude, LinkStatus::Error, Some("x".into()));
        let first = state.lock().unwrap().claude_link.clone().unwrap();
        set_link(&state, Provider::Claude, LinkStatus::Error, Some("x".into()));
        assert_eq!(state.lock().unwrap().claude_link.clone().unwrap(), first);
        set_link(&state, Provider::Claude, LinkStatus::Ok, None);
        assert_eq!(
            state.lock().unwrap().claude_link.as_ref().unwrap().status,
            LinkStatus::Ok
        );
    }
}
