//! One interactive terminal: a pseudo console (ConPTY on Windows, via `portable-pty`) running a
//! preset's program. Output goes to a callback from a reader thread; core answers ConPTY's
//! `ESC[6n` itself until a frontend that answers it (xterm.js) is attached.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::{DSR_REPLY, DsrWatcher};

#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("pty: {0}")]
    Pty(String),
    #[error("pty io: {0}")]
    Io(#[from] std::io::Error),
    #[error("closing the console took longer than {0:?}")]
    CloseTimeout(Duration),
}

pub type PtyResult<T> = Result<T, PtyError>;

fn pty_err(e: impl std::fmt::Display) -> PtyError {
    PtyError::Pty(e.to_string())
}

/// What to start. The command line is built in Rust from a preset, never taken from the UI.
pub struct SpawnSpec<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    /// Inherited variables to drop (see `opw_agents::env::is_stripped`).
    pub remove_env: &'a [String],
    /// Variables to add, e.g. the git guard from `opw_vcs::guard_env`.
    pub set_env: &'a [(String, String)],
    pub cols: u16,
    pub rows: u16,
}

pub struct PtySession {
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    answer_dsr: Arc<AtomicBool>,
    dsr_answered: Arc<Mutex<usize>>,
    reader: Option<JoinHandle<()>>,
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl PtySession {
    /// Start `spec.program` in a new pseudo console. `on_output` runs on the reader thread for
    /// every chunk the program writes.
    pub fn spawn(spec: &SpawnSpec<'_>, mut on_output: impl FnMut(&[u8]) + Send + 'static) -> PtyResult<Self> {
        let pair = native_pty_system()
            .openpty(size(spec.cols, spec.rows))
            .map_err(pty_err)?;
        let mut cmd = CommandBuilder::new(spec.program);
        cmd.args(spec.args);
        cmd.cwd(spec.cwd);
        for name in spec.remove_env {
            cmd.env_remove(name);
        }
        cmd.env("TERM", "xterm-256color");
        for (k, v) in spec.set_env {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).map_err(pty_err)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(pty_err)?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(pty_err)?));
        let answer_dsr = Arc::new(AtomicBool::new(true));
        let dsr_answered = Arc::new(Mutex::new(0usize));

        let thread = {
            let (writer, answer_dsr, dsr_answered) =
                (writer.clone(), answer_dsr.clone(), dsr_answered.clone());
            std::thread::spawn(move || {
                let mut watcher = DsrWatcher::default();
                let mut chunk = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let data = &chunk[..n];
                            let queries = watcher.feed(data);
                            if queries > 0 && answer_dsr.load(Ordering::Relaxed) {
                                if let Ok(mut w) = writer.lock() {
                                    for _ in 0..queries {
                                        let _ = w.write_all(DSR_REPLY);
                                    }
                                    let _ = w.flush();
                                }
                                if let Ok(mut c) = dsr_answered.lock() {
                                    *c += queries;
                                }
                            }
                            on_output(data);
                        }
                    }
                }
            })
        };

        Ok(Self {
            master: Some(pair.master),
            writer,
            child,
            answer_dsr,
            dsr_answered,
            reader: Some(thread),
        })
    }

    /// Stop answering `ESC[6n` once xterm.js is attached (it answers by itself).
    pub fn set_answer_dsr(&self, on: bool) {
        self.answer_dsr.store(on, Ordering::Relaxed);
    }

    /// How many cursor queries core has answered so far.
    pub fn dsr_answered(&self) -> usize {
        self.dsr_answered.lock().map(|c| *c).unwrap_or_default()
    }

    pub fn write(&self, data: &[u8]) -> PtyResult<()> {
        let mut w = self.writer.lock().map_err(pty_err)?;
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> PtyResult<()> {
        match &self.master {
            Some(m) => m.resize(size(cols, rows)).map_err(pty_err),
            None => Err(PtyError::Pty("closed".into())),
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// `Some(exit code)` once the program has exited.
    pub fn exit_code(&mut self) -> Option<u32> {
        self.child.try_wait().ok().flatten().map(|s| s.exit_code())
    }

    /// Kill the program and everything it started. Returns what the OS reported.
    pub fn kill_tree(&mut self) -> String {
        let report = match self.pid() {
            Some(pid) => kill_tree_os(pid),
            None => String::new(),
        };
        if self.exit_code().is_none() {
            let _ = self.child.kill();
        }
        report
    }

    /// Wait up to `timeout` for the program to exit.
    pub fn wait_exit(&mut self, timeout: Duration) -> Option<u32> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(code) = self.exit_code() {
                return Some(code);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Close the pseudo console and join the reader; returns how long it took.
    /// [`PtyError::CloseTimeout`] if it did not finish within `timeout` (the hang seen when
    /// `ESC[6n` goes unanswered); the work is then left on a background thread.
    pub fn close(mut self, timeout: Duration) -> PtyResult<Duration> {
        let started = Instant::now();
        let master = self.master.take();
        let reader = self.reader.take();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            drop(master);
            if let Some(r) = reader {
                let _ = r.join();
            }
            let _ = tx.send(());
        });
        rx.recv_timeout(timeout)
            .map(|()| started.elapsed())
            .map_err(|_| PtyError::CloseTimeout(timeout))
    }
}

#[cfg(windows)]
fn kill_tree_os(pid: u32) -> String {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|e| format!("taskkill failed: {e}"))
}

#[cfg(not(windows))]
fn kill_tree_os(_pid: u32) -> String {
    // Unix: Child::kill below; process groups come with the Phase 3 terminal work.
    String::new()
}
