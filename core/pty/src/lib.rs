//! Interactive terminals (Phase 3). Backend: `portable-pty` (ConPTY on Windows), pending
//! ADR 0003 once the claude/codex TUIs are tried in a pane (PLAN.md §12).
//!
//! Only fixed presets can be launched; the UI never sends a command line. The child
//! environment is filtered with `opw_agents::env::is_stripped`.

pub mod session;

pub use session::{PtyError, PtyResult, PtySession, SpawnSpec};

/// Terminal presets. The command line for each is built in Rust, never received from the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Claude,
    Codex,
    Shell,
}

/// Bytes kept per pane so a re-opened pane shows recent output.
pub const REPLAY_BUFFER_BYTES: usize = 256 * 1024;

/// ConPTY asks the host for the cursor position (DSR) as soon as it opens and blocks until it
/// gets an answer. Spike 2026-11-13 (portable-pty 0.9.0): unanswered, the child and even
/// closing the PTY hang; answered, output flows and exit is clean.
pub const DSR_QUERY: &[u8] = b"\x1b[6n";
/// Reply sent by core while no xterm.js is attached to the pane (xterm.js answers itself).
pub const DSR_REPLY: &[u8] = b"\x1b[1;1R";

/// Spots `ESC[6n` in PTY output, including a query split across two reads.
#[derive(Debug, Default)]
pub struct DsrWatcher {
    tail: Vec<u8>,
}

impl DsrWatcher {
    /// Number of queries completed by this chunk; reply once per query.
    pub fn feed(&mut self, chunk: &[u8]) -> usize {
        let mut buf = std::mem::take(&mut self.tail);
        buf.extend_from_slice(chunk);
        let count = buf.windows(DSR_QUERY.len()).filter(|w| *w == DSR_QUERY).count();
        let keep = buf.len().min(DSR_QUERY.len() - 1);
        self.tail = buf[buf.len() - keep..].to_vec();
        count
    }
}

/// Terminal output as plain text (CSI and OSC sequences removed), for logs and previews.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\x1b' => match it.next() {
                Some('[') => {
                    for d in it.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(d) = it.next() {
                        if d == '\x07' {
                            break;
                        }
                        if d == '\x1b' {
                            it.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' => out.push('\n'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_keeps_text_only() {
        let raw = "\x1b[2J\x1b[1;1H\x1b]0;title\x07Hello \x1b[38;5;208mClaude\x1b[0m\r\nok";
        assert_eq!(strip_ansi(raw), "Hello Claude\nok");
    }

    #[test]
    fn finds_queries_whole_and_split_without_double_counting() {
        let mut w = DsrWatcher::default();
        assert_eq!(w.feed(b"hello\x1b[6n"), 1);
        assert_eq!(w.feed(b"more"), 0);
        assert_eq!(w.feed(b"x\x1b["), 0);
        assert_eq!(w.feed(b"6ny"), 1);
        assert_eq!(w.feed(b"\x1b[6n\x1b[6n"), 2);
        assert_eq!(w.feed(b""), 0);
    }
}
