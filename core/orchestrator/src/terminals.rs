//! A Workspace console can outlive its view. Keep only a bounded replay, with at most one
//! output frame in flight per view so a noisy child cannot flood the WebView.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opw_agents::cli;
use opw_pty::{PtySession, REPLAY_BUFFER_BYTES, SpawnSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalKind {
    Claude,
    PowerShell,
    GitBash,
}

impl TerminalKind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::PowerShell => "powerShell",
            Self::GitBash => "gitBash",
        }
    }

    pub(crate) fn shell(self) -> Option<(PathBuf, Vec<String>)> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        match self {
            Self::Claude => None,
            Self::PowerShell => cli::find_in_path(&["pwsh.exe", "pwsh", "powershell.exe"], &path)
                .map(|exe| (exe, vec!["-NoLogo".into()])),
            Self::GitBash => git_bash().map(|exe| (exe, vec!["-i".into()])),
        }
    }
}

pub(crate) fn git_bash() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    if !cfg!(windows) {
        return cli::find_in_path(&["bash"], &path);
    }
    // Git adds cmd/git.exe to PATH, while its interactive Bash is under bin/.
    cli::find_in_path(&["git.exe"], &path)
        .into_iter()
        .flat_map(|git| {
            git.ancestors()
                .take(5)
                .map(|dir| dir.join("bin/bash.exe"))
                .collect::<Vec<_>>()
        })
        .chain(
            ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
                .into_iter()
                .filter_map(std::env::var_os)
                .flat_map(|dir| {
                    let dir = PathBuf::from(dir);
                    [
                        dir.join("Git/bin/bash.exe"),
                        dir.join("Programs/Git/bin/bash.exe"),
                    ]
                }),
        )
        .find(|exe| exe.is_file())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalFrame {
    bytes: Vec<u8>,
    replay: bool,
    skipped: bool,
}

pub type TerminalSink = Box<dyn Fn(TerminalFrame) -> bool + Send>;

struct Viewer {
    id: String,
    next: u64,
    replay_end: u64,
    waiting: bool,
    send: TerminalSink,
}

#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    end: u64,
    viewer: Option<Viewer>,
}

impl Output {
    fn start(&self) -> u64 {
        self.end - self.bytes.len() as u64
    }

    fn pump(&mut self) {
        let start = self.start();
        let Some(viewer) = &mut self.viewer else { return };
        if viewer.waiting || viewer.next == self.end {
            return;
        }
        let skipped = viewer.next < start;
        let from = viewer.next.max(start);
        let replay = from < viewer.replay_end;
        let end = (from + 16 * 1024).min(if replay { viewer.replay_end } else { self.end });
        let bytes = self.bytes.range((from - start) as usize..(end - start) as usize);
        viewer.waiting = (viewer.send)(TerminalFrame {
            bytes: bytes.copied().collect(),
            replay,
            skipped,
        });
        viewer.next = end;
    }
}

pub(crate) struct Pane {
    pub(crate) pty: PtySession,
    output: Arc<Mutex<Output>>,
}

impl Pane {
    pub(crate) fn spawn(spec: &SpawnSpec<'_>) -> opw_pty::PtyResult<Self> {
        let output = Arc::new(Mutex::new(Output::default()));
        let buffer = output.clone();
        let pty = PtySession::spawn(spec, move |data| {
            let mut out = buffer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let drop = (out.bytes.len() + data.len()).saturating_sub(REPLAY_BUFFER_BYTES);
            out.bytes.drain(..drop);
            out.bytes.extend(data);
            out.end += data.len() as u64;
            out.pump();
        })?;
        Ok(Self { pty, output })
    }

    pub(crate) fn attach(&self, view: String, send: TerminalSink) {
        let mut out = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        out.viewer = Some(Viewer {
            id: view,
            // Starting at zero lets the first replay frame say if older bytes were dropped.
            next: 0,
            replay_end: out.end,
            waiting: false,
            send,
        });
        self.pty.set_answer_dsr(false);
        out.pump();
    }

    pub(crate) fn owns(&self, view: &str) -> bool {
        self.output
            .lock()
            .ok()
            .is_some_and(|out| out.viewer.as_ref().is_some_and(|v| v.id == view))
    }

    pub(crate) fn ack(&self, view: &str) {
        let mut out = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(v) = out.viewer.as_mut().filter(|v| v.id == view) {
            v.waiting = false;
            out.pump();
        }
    }

    pub(crate) fn detach(&self, view: &str) {
        let mut out = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if out.viewer.as_ref().is_some_and(|v| v.id == view) {
            self.pty.set_answer_dsr(true);
            out.viewer = None;
        }
    }

    pub(crate) fn close(mut self) -> opw_pty::PtyResult<Duration> {
        self.pty.set_answer_dsr(true);
        if self.pty.exit_code().is_none() {
            self.pty.kill_tree();
        }
        self.pty.close(Duration::from_secs(3))
    }
}

pub(crate) fn statusline_settings() -> String {
    // Only the packaged executable is invoked. Read its path from the environment so spaces,
    // quotes and shell metacharacters in the install directory stay inside a single argument.
    serde_json::json!({"statusLine": {
        "type": "command", "command": r#""$OPW_STATUSLINE_EXE" --statusline-hook"#
    }})
    .to_string()
}
