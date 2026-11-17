//! Run one AI CLI process: argument array (never a shell string), prompt on stdin, stdout read
//! line by line. Stopped on request or after a timeout, together with its child processes.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::env::{Account, is_stripped};
use crate::procs::{self, Sweep};

/// How often the reader wakes up to look at the stop flag and the deadline.
const POLL: Duration = Duration::from_millis(200);
/// After a kill, how long to wait for the pipes to close before giving up on them.
const DRAIN_AFTER_KILL: Duration = Duration::from_secs(5);
/// Bytes of stderr kept for the error message.
const STDERR_TAIL: usize = 4096;
/// After the process itself exits, how long its last output may take to arrive before what it
/// left running (holding the pipes open) is killed.
const AFTER_EXIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The prompt; `None` = no stdin (the prompt is an argument, e.g. `/review-local`).
    pub stdin: Option<String>,
    /// Added after the inherited environment is filtered (git guard, API key).
    pub env: Vec<(String, String)>,
    pub account: Account,
    pub timeout: Duration,
    /// Look for what the process left running when it ends and kill it (dev servers, Chrome).
    /// Costs a PowerShell query or two, so short commands that start nothing skip it.
    pub sweep: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutcome {
    /// Exit code; `None` when killed.
    pub code: Option<i32>,
    /// Last few KB of stderr (errors such as "not logged in" or an unknown flag).
    pub stderr_tail: String,
    /// The stop flag ended it.
    pub stopped: bool,
    pub timed_out: bool,
    /// Processes the run left behind (dev servers, Chrome, …), killed when it ended.
    pub sweep: Sweep,
}

impl RunOutcome {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.stopped && !self.timed_out
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

/// Kill the process and everything it started (Claude runs Bash, Codex runs PowerShell).
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut taskkill = Command::new("taskkill");
        taskkill
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut taskkill);
        let _ = taskkill.status();
    }
    let _ = child.kill();
}

/// Run `spec`, calling `on_line` for every stdout line (without the line ending). Blocks until
/// the process ends, `stop` turns true, or the timeout passes.
pub fn run(spec: &RunSpec, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> std::io::Result<RunOutcome> {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0");
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if is_stripped(&name, spec.account) {
            cmd.env_remove(&key);
        }
    }
    cmd.envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    hide_window(&mut cmd);
    let started_ms = procs::now_ms();
    let mut child = cmd.spawn()?;
    let root = child.id();
    let watch = spec.sweep.then(|| procs::Watch::start(root, started_ms));
    let mut known: Option<HashMap<u32, u64>> = None;
    let sweep_now = |known: &mut Option<HashMap<u32, u64>>, watch: &mut Option<procs::Watch>| {
        let seen = known.get_or_insert_with(|| watch.take().map(procs::Watch::finish).unwrap_or_default());
        procs::sweep(root, started_ms, seen)
    };

    if let (Some(prompt), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
        std::thread::Builder::new()
            .name("opw-agent-stdin".into())
            .spawn(move || {
                let _ = stdin.write_all(prompt.as_bytes());
                // dropping stdin sends EOF: the CLI starts working
            })?;
    }
    let stderr = child.stderr.take();
    let stderr_thread = std::thread::Builder::new()
        .name("opw-agent-stderr".into())
        .spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut s) = stderr {
                let _ = s.read_to_end(&mut buf);
            }
            let start = buf.len().saturating_sub(STDERR_TAIL);
            String::from_utf8_lossy(&buf[start..]).trim().to_owned()
        })?;
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        std::thread::Builder::new()
            .name("opw-agent-stdout".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&buf);
                            let line = line.trim_end_matches(['\r', '\n']);
                            if tx.send(line.to_owned()).is_err() {
                                return;
                            }
                        }
                    }
                }
            })?;
    }

    let deadline = Instant::now() + spec.timeout;
    let mut outcome = RunOutcome::default();
    let mut killed_at: Option<Instant> = None;
    let mut exited_at: Option<Instant> = None;
    let mut watch = watch;
    let mut swept = !spec.sweep;
    loop {
        match rx.recv_timeout(POLL) {
            Ok(line) => on_line(&line),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        // the CLI is gone but something it started keeps stdout open: clean up, then the pipe
        // closes and the loop ends
        if exited_at.is_none() && killed_at.is_none() && matches!(child.try_wait(), Ok(Some(_))) {
            exited_at = Some(Instant::now());
        }
        if !swept && exited_at.is_some_and(|at| at.elapsed() > AFTER_EXIT) {
            outcome.sweep = sweep_now(&mut known, &mut watch);
            swept = true;
        }
        match killed_at {
            Some(at) if at.elapsed() > DRAIN_AFTER_KILL => break,
            Some(_) => {}
            None => {
                let stop_now = stop.load(Ordering::Relaxed);
                let late = Instant::now() >= deadline;
                if stop_now || late {
                    outcome.stopped = stop_now;
                    outcome.timed_out = late && !stop_now;
                    kill_tree(&mut child);
                    killed_at = Some(Instant::now());
                }
            }
        }
    }
    let status = child.wait()?;
    if !swept {
        // what the run started and left running (a dev server, Chrome) goes with it
        outcome.sweep = sweep_now(&mut known, &mut watch);
    }
    outcome.code = if killed_at.is_some() { None } else { status.code() };
    outcome.stderr_tail = stderr_thread.join().unwrap_or_default();
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn spec(program: &str, args: &[&str], stdin: Option<&str>, timeout: Duration) -> RunSpec {
        RunSpec {
            program: PathBuf::from(program),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            cwd: std::env::temp_dir(),
            stdin: stdin.map(str::to_owned),
            env: vec![("OPW_TEST_VAR".into(), "1".into())],
            account: Account::Subscription,
            timeout,
            sweep: false,
        }
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn prompt_goes_through_stdin_and_lines_come_back() {
        if !git_available() {
            return;
        }
        let mut lines = Vec::new();
        let out = run(
            &spec(
                "git",
                &["hash-object", "--stdin"],
                Some("hello\n"),
                Duration::from_secs(30),
            ),
            &AtomicBool::new(false),
            &mut |l| lines.push(l.to_owned()),
        )
        .unwrap();
        assert!(out.success(), "{out:?}");
        // `git hash-object` of "hello\n"
        assert_eq!(lines, ["ce013625030ba8dba906f756967f9e9ca394464a"]);
    }

    #[test]
    fn failures_keep_stderr() {
        if !git_available() {
            return;
        }
        let out = run(
            &spec("git", &["no-such-command-xyz"], None, Duration::from_secs(30)),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert!(!out.success());
        assert!(
            out.stderr_tail.contains("no-such-command-xyz"),
            "{}",
            out.stderr_tail
        );
        assert!(
            run(
                &spec("opw-no-such-program-xyz", &[], None, Duration::from_secs(5)),
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .is_err()
        );
    }

    /// A long-running system command: `ping` waits ~1 s between echoes on every platform.
    fn long_command() -> (&'static str, Vec<&'static str>) {
        if cfg!(windows) {
            ("ping", vec!["-n", "30", "127.0.0.1"])
        } else {
            ("sleep", vec!["30"])
        }
    }

    #[test]
    fn stop_and_timeout_end_the_process_quickly() {
        let (program, args) = long_command();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            flag.store(true, Ordering::Relaxed);
        });
        let began = Instant::now();
        let out = run(
            &spec(program, &args, None, Duration::from_secs(60)),
            &stop,
            &mut |_| {},
        )
        .unwrap();
        assert!(out.stopped && !out.timed_out && out.code.is_none(), "{out:?}");
        assert!(began.elapsed() < Duration::from_secs(15));

        let began = Instant::now();
        let out = run(
            &spec(program, &args, None, Duration::from_millis(500)),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert!(out.timed_out && !out.stopped, "{out:?}");
        assert!(began.elapsed() < Duration::from_secs(15));
    }
}
