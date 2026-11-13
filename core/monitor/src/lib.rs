//! Session monitoring without reading the screen (Phase 3, PLAN.md §5).
//!
//! Sources, all owned by the CLIs and read-only for the app:
//! - `~/.claude/sessions/<pid>.json`: busy / idle / waiting
//! - `~/.claude/projects/**/<session>.jsonl`: tools, turn end, `compact_boundary`
//! - `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`: task events, `token_count`
//! - `<data_dir>/events/*.jsonl`: written by `opw-hook`
//!
//! Files are tailed by byte offset; a missing or unknown source degrades the view, never crashes.

/// Where a session's state comes from, used for labelling in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorSource {
    ClaudeRegistry,
    ClaudeTranscript,
    CodexRollout,
    HookEvent,
    StreamJson,
}
