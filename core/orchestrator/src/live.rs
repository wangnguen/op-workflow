//! Live data (Phase 1, read-only): the OpenProject inbox and AI quota, refreshed on background
//! threads so IPC calls only read a cache and never wait on the network.
//!
//! - Tasks: `HttpTracker::inbox` every `tracker.pollMin` minutes, or right away on `refresh`.
//! - Codex quota: one long-lived `codex app-server` (JSON-RPC over stdio), polled every
//!   `quota.codexPollSec` seconds; restarted if it dies.
//! - Claude quota: a minimal `claude -p` probe (~735 Haiku tokens) in an empty folder every
//!   `quota.claudeProbeMin` minutes, reading its `rate_limit_event` (PLAN.md §7).
//!
//! Nothing here writes to OpenProject.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opw_agents::cli;
use opw_agents::env::{Account, is_stripped};
use opw_domain::view::QuotaSnapshot;
use opw_tracker::{HttpTracker, TrackerApi, WorkPackage};
use serde_json::{Value, json};

const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const PROBE_TIMEOUT: Duration = Duration::from_secs(90);
const RESTART_BACKOFF: Duration = Duration::from_secs(60);

/// Cached live state. The token stays inside the process and is never serialized.
#[derive(Clone, Default)]
pub struct LiveState {
    /// OpenProject URL (empty until entered in Settings).
    pub base_url: String,
    pub token: Option<String>,
    pub nx_user: Option<String>,
    /// `None` until the first successful fetch.
    pub tasks: Option<Vec<WorkPackage>>,
    pub last_sync: Option<String>,
    pub tasks_error: Option<String>,
    pub claude: Option<QuotaSnapshot>,
    pub codex: Option<QuotaSnapshot>,
    pub quota_error: Option<String>,
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

#[derive(Debug, Clone)]
pub struct LiveSettings {
    pub poll: Duration,
    pub codex_poll: Duration,
    pub claude_probe: Duration,
    pub claude_probe_model: String,
    /// Empty folder for the Claude probe, so no CLAUDE.md is loaded.
    pub probe_dir: PathBuf,
}

#[derive(Debug)]
pub struct Live {
    state: Arc<Mutex<LiveState>>,
    wake_tasks: Sender<()>,
    pub settings: LiveSettings,
}

fn now() -> String {
    opw_quota::now_rfc3339()
}

impl Live {
    /// Start the background threads. `token` is the saved OpenProject token, if any.
    pub fn start(settings: LiveSettings, base_url: String, token: Option<String>) -> Self {
        let state = Arc::new(Mutex::new(LiveState {
            base_url,
            token,
            ..LiveState::default()
        }));
        let (wake_tasks, wake_rx) = mpsc::channel();
        spawn_named("opw-tasks", {
            let (state, s) = (state.clone(), settings.clone());
            move || tasks_loop(&state, &s, &wake_rx)
        });
        spawn_named("opw-codex-quota", {
            let (state, every) = (state.clone(), settings.codex_poll);
            move || codex_loop(&state, every)
        });
        spawn_named("opw-claude-quota", {
            let (state, s) = (state.clone(), settings.clone());
            move || claude_loop(&state, &s)
        });
        Self {
            state,
            wake_tasks,
            settings,
        }
    }

    pub fn snapshot(&self) -> LiveState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Use a token that the caller has already checked with `HttpTracker::me`.
    pub fn set_token(&self, base_url: String, token: String, user: String) {
        if let Ok(mut s) = self.state.lock() {
            s.base_url = base_url;
            s.token = Some(token);
            s.nx_user = Some(user);
            s.tasks = None;
            s.tasks_error = None;
        }
        self.refresh();
    }

    pub fn clear_token(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.token = None;
            s.nx_user = None;
            s.tasks = None;
            s.tasks_error = None;
            s.last_sync = None;
        }
    }

    /// Fetch tasks now instead of waiting for the next poll.
    pub fn refresh(&self) {
        let _ = self.wake_tasks.send(());
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

fn tasks_loop(state: &Mutex<LiveState>, settings: &LiveSettings, wake: &Receiver<()>) {
    loop {
        let (base_url, token, need_user) = state
            .lock()
            .map(|s| (s.base_url.clone(), s.token.clone(), s.nx_user.is_none()))
            .unwrap_or_default();
        if let Some(token) = token.filter(|_| !base_url.is_empty()) {
            let tracker = HttpTracker::new(&base_url, &token);
            let user = need_user.then(|| tracker.me().ok()).flatten();
            let result = tracker.inbox();
            update(state, |s| {
                // the token may have been cleared or replaced while we were fetching
                if s.token.as_deref() != Some(token.as_str()) {
                    return;
                }
                if user.is_some() {
                    s.nx_user = user;
                }
                match result {
                    Ok(tasks) => {
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

// ---------- child processes ----------

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    for (k, _) in std::env::vars() {
        if is_stripped(&k, Account::Subscription) {
            cmd.env_remove(k);
        }
    }
    cmd
}

/// Lines from a child's stdout, read on their own thread so callers can time out.
fn line_reader(child: &mut Child) -> Option<Receiver<String>> {
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

fn codex_loop(state: &Mutex<LiveState>, every: Duration) {
    loop {
        match cli::find_codex() {
            Some(exe) => {
                if let Err(e) = run_app_server(&exe, state, every) {
                    update(state, |s| s.quota_error = Some(format!("codex app-server: {e}")));
                }
            }
            None => update(state, |s| s.quota_error = Some("codex not found on PATH".into())),
        }
        std::thread::sleep(RESTART_BACKOFF);
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

fn store_codex(state: &Mutex<LiveState>, result: &Value) {
    if let Some(snap) = opw_quota::from_codex_app_server(result, &now()) {
        update(state, |s| {
            s.codex = Some(snap);
            if s.quota_error.as_deref().is_some_and(|e| e.starts_with("codex")) {
                s.quota_error = None;
            }
        });
    }
}

fn run_app_server(exe: &Path, state: &Mutex<LiveState>, every: Duration) -> Result<(), String> {
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
            store_codex(state, &result);
            id += 1;
            // Keep reading notifications until the next poll.
            let next = Instant::now() + every;
            while let Some(left) = next.checked_duration_since(Instant::now()) {
                match rx.recv_timeout(left) {
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

fn claude_loop(state: &Mutex<LiveState>, settings: &LiveSettings) {
    loop {
        match cli::find_claude() {
            Some(exe) => match probe_claude(&exe, settings) {
                Ok(Some(snap)) => update(state, |s| {
                    s.claude = Some(snap);
                    if s.quota_error.as_deref().is_some_and(|e| e.starts_with("claude")) {
                        s.quota_error = None;
                    }
                }),
                Ok(None) => update(state, |s| {
                    s.quota_error = Some("claude probe: no rate_limit_event".into())
                }),
                Err(e) => update(state, |s| s.quota_error = Some(format!("claude probe: {e}"))),
            },
            None => update(state, |s| s.quota_error = Some("claude not found on PATH".into())),
        }
        std::thread::sleep(settings.claude_probe);
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
