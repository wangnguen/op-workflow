//! `op-workflow-desktop --pty-selftest`: the PTY spike (PLAN.md §12) run from inside the app
//! binary, because phần mềm diệt virus quarantined the standalone spike exes.
//!
//! For `claude` and `codex`: open the real TUI in an empty folder through ConPTY, answer the
//! cursor query, resize, kill the whole process tree and close the console. No prompt is
//! typed, so no tokens are used and nothing is trusted or saved by the CLIs.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opw_agents::cli;
use opw_agents::env::{Account, is_stripped};
use opw_pty::{PtySession, SpawnSpec, strip_ansi};

const RUN_FOR: Duration = Duration::from_secs(8);
const AFTER_RESIZE: Duration = Duration::from_secs(2);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const SAMPLE_CHARS: usize = 160;

/// Run the self-test for both CLIs and return a plain-text report.
pub fn run_pty() -> String {
    let cwd = std::env::temp_dir().join("opw-pty-selftest");
    let _ = std::fs::create_dir_all(&cwd);
    let remove_env: Vec<String> = std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| is_stripped(k, Account::Subscription))
        .collect();

    let mut out = String::new();
    let _ = writeln!(
        out,
        "op-workflow PTY self-test (portable-pty, {})",
        std::env::consts::OS
    );
    let _ = writeln!(out, "folder: {}", cwd.display());
    for (name, exe) in [("claude", cli::find_claude()), ("codex", cli::find_codex())] {
        let _ = writeln!(out);
        match exe {
            Some(exe) => out.push_str(&one(name, &exe, &cwd, &remove_env)),
            None => {
                let _ = writeln!(out, "{name}: not found on PATH -> SKIPPED");
            }
        }
    }
    out
}

/// Save the report next to the app data (release builds have no console).
pub fn save(report: &str) -> Option<PathBuf> {
    let dir = opw_config::data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("pty-selftest.txt");
    std::fs::write(&path, report).ok()?;
    Some(path)
}

fn one(name: &str, exe: &Path, cwd: &Path, remove_env: &[String]) -> String {
    let mut r = String::new();
    let _ = writeln!(r, "{name}: {}", exe.display());

    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let first = Arc::new(Mutex::new(None::<Duration>));
    let started = Instant::now();
    let spec = SpawnSpec {
        program: exe,
        args: &[],
        cwd,
        remove_env,
        set_env: &[],
        cols: 120,
        rows: 30,
    };
    let sink = {
        let (output, first) = (output.clone(), first.clone());
        move |data: &[u8]| {
            if let Ok(mut f) = first.lock() {
                f.get_or_insert_with(|| started.elapsed());
            }
            if let Ok(mut o) = output.lock() {
                o.extend_from_slice(data);
            }
        }
    };
    let mut session = match PtySession::spawn(&spec, sink) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(r, "  spawn failed: {e}\n  RESULT: FAIL");
            return r;
        }
    };
    let bytes = |o: &Arc<Mutex<Vec<u8>>>| o.lock().map(|v| v.len()).unwrap_or_default();

    std::thread::sleep(RUN_FOR);
    let first_ms = first.lock().ok().and_then(|f| *f).map(|d| d.as_millis());
    let before_resize = bytes(&output);
    let crashed = session.exit_code();
    let _ = writeln!(
        r,
        "  first output: {}; {before_resize} bytes in {}s; ESC[6n answered {}x",
        first_ms.map_or("none".to_owned(), |ms| format!("{ms} ms")),
        RUN_FOR.as_secs(),
        session.dsr_answered()
    );

    let resized = session.resize(100, 40);
    std::thread::sleep(AFTER_RESIZE);
    let alive_after_resize = session.exit_code().is_none();
    let grew = bytes(&output).saturating_sub(before_resize);
    let _ = writeln!(
        r,
        "  resize 120x30 -> 100x40: {}; {}; +{grew} bytes after resize",
        if resized.is_ok() { "ok" } else { "error" },
        if alive_after_resize {
            "still running"
        } else {
            "exited"
        }
    );

    let kill_report = session.kill_tree();
    let killed = kill_report.lines().filter(|l| l.starts_with("SUCCESS")).count();
    let exited = session.wait_exit(Duration::from_secs(3));
    let _ = writeln!(
        r,
        "  kill tree: {killed} process(es) terminated; exited: {}",
        exited.map_or("NO".to_owned(), |c| format!("yes (code {c})"))
    );

    let closed = session.close(CLOSE_TIMEOUT);
    let _ = writeln!(
        r,
        "  close console: {}",
        match &closed {
            Ok(d) => format!("{} ms", d.as_millis()),
            Err(e) => format!("HANG ({e})"),
        }
    );

    let text = output
        .lock()
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default();
    let plain = opw_context::redact(&strip_ansi(&text), &[]);
    let sample: String = plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SAMPLE_CHARS)
        .collect();
    let _ = writeln!(r, "  screen: {sample:?}");

    let mut problems = Vec::new();
    if first_ms.is_none() || before_resize < 200 {
        problems.push("little or no output");
    }
    if crashed.is_some() {
        problems.push("exited before the test ended");
    }
    if resized.is_err() || !alive_after_resize {
        problems.push("resize");
    }
    if exited.is_none() {
        problems.push("still running after kill");
    }
    if closed.is_err() {
        problems.push("console close hang");
    }
    let _ = writeln!(
        r,
        "  RESULT: {}",
        if problems.is_empty() {
            "OK".to_owned()
        } else {
            format!("FAIL ({})", problems.join(", "))
        }
    );
    r
}

/// `op-workflow-desktop --live-check`: start the live threads, wait until both quota sources
/// answered (or `timeout`), and report what the Settings dialog would show. Costs one Claude
/// probe (~735 Haiku tokens). The token itself is never printed.
pub fn run_live(timeout: Duration) -> String {
    let service = crate::AppService::start();
    let started = Instant::now();
    loop {
        let s = service.settings();
        let quota_done = s.claude_quota_at.is_some() && s.codex_quota_at.is_some();
        let tasks_done = !s.nx_token_set || s.nx_last_sync.is_some() || s.nx_error.is_some();
        if (quota_done && tasks_done) || started.elapsed() >= timeout {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let settings = service.settings();
    let board = service.board_snapshot();
    let mut r = String::new();
    let _ = writeln!(r, "op-workflow live check ({} s)", started.elapsed().as_secs());
    if settings.nx_base_url.is_empty() {
        let _ = writeln!(r, "OpenProject: URL not set yet (Settings)");
    } else if !settings.nx_token_set {
        // Reachability only: no token is sent, so the server must answer 401.
        let probe = opw_tracker::HttpTracker::new(&settings.nx_base_url, "").me();
        let _ = writeln!(
            r,
            "OpenProject without token: {}",
            match probe {
                Err(opw_tracker::TrackerError::Http { status: 401, .. }) =>
                    "reachable (HTTP 401 as expected)".to_owned(),
                Err(e) => format!("{e}"),
                Ok(_) => "unexpected success without token".to_owned(),
            }
        );
    }
    let _ = writeln!(
        r,
        "{}",
        serde_json::to_string_pretty(&settings).unwrap_or_default()
    );
    let _ = writeln!(r, "board mode: {:?}; tasks: {}", board.mode, board.tasks.len());
    for q in &board.quota {
        let windows: Vec<String> = q
            .windows
            .iter()
            .map(|w| format!("{:?} {:.0}% used", w.kind, w.used_pct))
            .collect();
        let _ = writeln!(r, "quota {:?} ({}): {}", q.provider, q.source, windows.join(", "));
    }
    r
}
