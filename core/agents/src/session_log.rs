//! The conversation of one Claude Code session as the Workspace chat shows it: what the user
//! typed, what the AI wrote, each tool call with a shortened result, interruptions and compacts.
//!
//! The same lines arrive from two places: the session's transcript
//! (`~/.claude/projects/<folder>/<id>.jsonl`, read when a session is opened) and the stream-json
//! output of the process the app runs for it (appended live). Both carry `user` / `assistant`
//! messages with the same `message.content` blocks, so one builder reads both. Unknown lines are
//! skipped, so a newer CLI that adds line types still shows what this version understands.
//!
//! Subagents (the Agent / Task tool) show under the call they run for, as the VS Code extension
//! shows them: live, their messages carry `parent_tool_use_id`; on disk, each has its own file
//! `<id>/subagents/agent-<n>.jsonl` with a `.meta.json` naming that call (`toolUseId`, checked on
//! CLI 2.1.329). Since 2.1.32x the Agent tool runs in the background by default: the call returns
//! "Async agent launched" at once and the result comes later as a task notification (a `system`
//! `task_notification` line live, a `<task-notification>` user message in the transcript).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::claude::cut;
use crate::transcript::{Item, ItemKind, TEXT_MAX, claude_tool_text, result_text, str_at};

/// Items kept; the oldest go first (the chat says how many were left out).
pub const ITEMS_MAX: usize = 600;
/// How many old items go at once when the list is full, so indices do not shift on every line.
const DROP_STEP: usize = 100;
/// `tool` of an [`ItemKind::End`] item: the user stopped the turn.
pub const END_INTERRUPTED: &str = "interrupted";
/// `tool` of an [`ItemKind::End`] item: the context was compacted.
pub const END_COMPACTED: &str = "compacted";

/// A subagent's items kept under its call (the newest).
pub const SUB_ITEMS_MAX: usize = 200;
/// How the tool result of an Agent call that went to the background starts.
const ASYNC_LAUNCHED: &str = "Async agent launched";
/// The user message the CLI adds when a background task ends.
const TASK_NOTIFICATION: &str = "<task-notification>";

/// Texts the CLI writes as a user message when a turn is stopped.
const INTERRUPT_TEXTS: [&str; 2] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];

#[derive(Debug, Clone, Default)]
pub struct SessionLog {
    items: Vec<Item>,
    /// tool_use id → index in `items`
    by_id: HashMap<String, usize>,
    /// Items left out at the start.
    dropped: usize,
    /// Lowest index changed since [`SessionLog::take_changed`] was last called.
    changed: Option<usize>,
    /// The title the CLI gave the session (`ai-title`) or the user set (`custom-title`).
    pub title: Option<String>,
    /// Subagents' own logs, by the Agent / Task call they run under.
    subs: HashMap<String, SessionLog>,
}

impl SessionLog {
    /// Read a whole transcript file.
    pub fn from_transcript(text: &str) -> Self {
        let mut log = Self::default();
        for line in text.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                log.apply(&v);
            }
        }
        log.changed = None;
        log
    }

    /// A transcript file with its subagents (`<id>/subagents/` next to it).
    pub fn read_file(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut log = Self::from_transcript(&text);
        log.attach_subagents(&subagents_dir(path));
        log.changed = None;
        Ok(log)
    }

    /// Each subagent file in `dir` under the call it ran for (its `.meta.json` says which).
    fn attach_subagents(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".meta.json"))
            else {
                continue;
            };
            let Some(meta) = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            else {
                continue;
            };
            let tool_id = str_at(&meta, "/toolUseId").to_owned();
            if tool_id.is_empty() || !self.by_id.contains_key(&tool_id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(dir.join(format!("{stem}.jsonl"))) else {
                continue;
            };
            let mut sub = Self::default();
            for line in text.lines() {
                if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                    sub.apply_line(&v);
                }
            }
            self.subs.insert(tool_id.clone(), sub);
            self.sync_sub(&tool_id);
            if meta.get("finished").and_then(Value::as_bool) == Some(true) {
                if let Some(item) = self.by_id.get(&tool_id).and_then(|&at| self.items.get_mut(at)) {
                    item.running = false;
                }
            }
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// The lowest item index that changed since the last call (`None` = nothing changed).
    pub fn take_changed(&mut self) -> Option<usize> {
        self.changed.take()
    }

    fn mark(&mut self, at: usize) {
        self.changed = Some(self.changed.map_or(at, |c| c.min(at)));
    }

    fn push(&mut self, item: Item) -> usize {
        if self.items.len() >= ITEMS_MAX {
            let n = DROP_STEP.min(self.items.len());
            self.items.drain(..n);
            self.dropped += n;
            self.by_id.retain(|_, at| {
                if *at < n {
                    false
                } else {
                    *at -= n;
                    true
                }
            });
            // everything moved: the whole list is new for whoever follows it
            self.changed = Some(0);
        }
        self.items.push(item);
        let at = self.items.len() - 1;
        self.mark(at);
        at
    }

    fn last_is(&self, kind: ItemKind, tool: &str) -> bool {
        self.items
            .last()
            .is_some_and(|i| i.kind == kind && i.tool.as_deref() == Some(tool))
    }

    fn end(&mut self, what: &str) {
        if !self.last_is(ItemKind::End, what) {
            let mut item = Item::new(ItemKind::End, String::new());
            item.tool = Some(what.to_owned());
            self.push(item);
        }
    }

    /// Apply one transcript line or one stream-json message.
    pub fn apply(&mut self, v: &Value) {
        // a subagent's own message, live: under the Agent / Task call it runs for
        if let Some(parent) = v
            .get("parent_tool_use_id")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
        {
            let parent = parent.to_owned();
            self.subs.entry(parent.clone()).or_default().apply_line(v);
            self.sync_sub(&parent);
            return;
        }
        // a subagent's line in the main transcript (older CLIs): its own file is read instead
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        self.apply_line(v);
    }

    /// Copy a subagent's items onto its call (what it was asked is on the call already).
    fn sync_sub(&mut self, tool_id: &str) {
        let (Some(&at), Some(sub)) = (self.by_id.get(tool_id), self.subs.get(tool_id)) else {
            return;
        };
        let shown: Vec<Item> = sub
            .items
            .iter()
            .filter(|i| i.kind != ItemKind::User)
            .cloned()
            .collect();
        let from = shown.len().saturating_sub(SUB_ITEMS_MAX);
        if let Some(item) = self.items.get_mut(at) {
            item.children = shown[from..].to_vec();
        }
        self.mark(at);
    }

    /// A background task ended: its call gets the result and stops showing as running.
    fn task_done(&mut self, tool_id: &str, status: &str, text: &str) {
        let Some(&at) = self.by_id.get(tool_id) else {
            return;
        };
        if let Some(item) = self.items.get_mut(at) {
            item.running = false;
            if !text.trim().is_empty() {
                item.set_output(text);
            }
            item.failed |= status == "failed";
        }
        self.mark(at);
    }

    fn task_started(&mut self, tool_id: &str) {
        if let Some(&at) = self.by_id.get(tool_id) {
            if let Some(item) = self.items.get_mut(at) {
                item.running = true;
            }
            self.mark(at);
        }
    }

    fn apply_line(&mut self, v: &Value) {
        match str_at(v, "/type") {
            "user" => self.user(v),
            "assistant" => self.assistant(v),
            "system" => match str_at(v, "/subtype") {
                "compact_boundary" => self.end(END_COMPACTED),
                "permission_denied" => self.denied(v),
                "task_started" => self.task_started(str_at(v, "/tool_use_id")),
                "task_notification" => {
                    let report = v.get("summary_is_report").and_then(Value::as_bool) == Some(true);
                    let text = if report { str_at(v, "/summary") } else { "" };
                    self.task_done(str_at(v, "/tool_use_id"), str_at(v, "/status"), text);
                }
                _ => {}
            },
            "result" => self.result(v),
            "ai-title" => self.set_title(str_at(v, "/aiTitle")),
            "custom-title" => self.set_title(str_at(v, "/customTitle")),
            _ => {}
        }
    }

    fn set_title(&mut self, title: &str) {
        let title = title.trim();
        if !title.is_empty() {
            self.title = Some(title.to_owned());
        }
    }

    fn user(&mut self, v: &Value) {
        if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let content = v.pointer("/message/content").unwrap_or(&Value::Null);
        let mut texts: Vec<String> = Vec::new();
        match content {
            Value::String(s) => texts.push(s.clone()),
            Value::Array(blocks) => {
                for block in blocks {
                    match str_at(block, "/type") {
                        "text" => texts.push(str_at(block, "/text").to_owned()),
                        "image" => texts.push("[ảnh]".to_owned()),
                        "tool_result" => self.tool_result(block),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        let text = texts
            .iter()
            .filter_map(|t| user_text(t))
            .collect::<Vec<_>>()
            .join("\n\n");
        if text.is_empty() {
            return;
        }
        if INTERRUPT_TEXTS.contains(&text.as_str()) {
            self.end(END_INTERRUPTED);
            return;
        }
        // a background task ended (the transcript's copy of the notification): not the user
        if text.starts_with(TASK_NOTIFICATION) {
            let tag = |name: &str| {
                between(&text, &format!("<{name}>"), &format!("</{name}>"))
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            };
            let result = Some(tag("result"))
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| tag("summary"));
            self.task_done(&tag("tool-use-id"), &tag("status"), &result);
            return;
        }
        // the echo of a message already shown (a replayed message seen twice)
        if self
            .items
            .last()
            .is_some_and(|i| i.kind == ItemKind::User && i.text == text)
        {
            return;
        }
        let mut item = Item::new(ItemKind::User, cut(&text, TEXT_MAX));
        item.message_id = v["uuid"].as_str().map(str::to_owned);
        self.push(item);
    }

    fn tool_result(&mut self, block: &Value) {
        let text = result_text(block.get("content").unwrap_or(&Value::Null));
        let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        if let Some(&at) = self.by_id.get(str_at(block, "/tool_use_id")) {
            if let Some(item) = self.items.get_mut(at) {
                if text.trim_start().starts_with(ASYNC_LAUNCHED) {
                    // the subagent went to the background: what the CLI says here is for the AI
                    // only; the result comes with the task notification
                    item.running = true;
                } else {
                    item.set_output(&text);
                }
                item.failed |= failed;
            }
            self.mark(at);
        }
    }

    fn assistant(&mut self, v: &Value) {
        for block in v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match str_at(block, "/type") {
                "text" => {
                    let text = str_at(block, "/text").trim();
                    if !text.is_empty() {
                        self.push(Item::new(ItemKind::Text, cut(text, TEXT_MAX)));
                    }
                }
                "thinking" => {
                    let text = str_at(block, "/thinking").trim();
                    if !text.is_empty() {
                        self.push(Item::new(ItemKind::Thinking, cut(text, TEXT_MAX)));
                    }
                }
                "tool_use" => {
                    let id = str_at(block, "/id").to_owned();
                    // the same block again (a message seen in the transcript and the stream)
                    if !id.is_empty() && self.by_id.contains_key(&id) {
                        continue;
                    }
                    let name = str_at(block, "/name");
                    let input = block.get("input").unwrap_or(&Value::Null);
                    let mut item = Item::tool(name, claude_tool_text(name, input));
                    item.tool_id = Some(id.clone()).filter(|i| !i.is_empty());
                    let at = self.push(item);
                    if !id.is_empty() {
                        self.by_id.insert(id, at);
                    }
                }
                _ => {}
            }
        }
    }

    fn denied(&mut self, v: &Value) {
        let reason = [str_at(v, "/decision_reason"), str_at(v, "/message")]
            .into_iter()
            .map(str::trim)
            .find(|s| !s.is_empty())
            .unwrap_or("bị từ chối");
        let reason = cut(reason, 300);
        if let Some(&at) = self.by_id.get(str_at(v, "/tool_use_id")) {
            if let Some(item) = self.items.get_mut(at) {
                item.denied = Some(reason);
                item.failed = true;
            }
            self.mark(at);
        }
    }

    fn result(&mut self, v: &Value) {
        if v.get("is_error").and_then(Value::as_bool) != Some(true) {
            return;
        }
        if str_at(v, "/subtype") == "error_during_execution" {
            // a stopped turn (the transcript has "[Request interrupted by user]" for it)
            self.end(END_INTERRUPTED);
            return;
        }
        let text = [str_at(v, "/result"), str_at(v, "/subtype")]
            .into_iter()
            .map(str::trim)
            .find(|s| !s.is_empty())
            .unwrap_or("lỗi")
            .to_owned();
        self.push(Item::new(ItemKind::Error, cut(&text, TEXT_MAX)));
    }
}

/// What a user message shows: the text without the wrappers the CLI adds, a slash command as
/// `/name args`, nothing for reminders and command output.
fn user_text(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty()
        || t.starts_with("<system-reminder>")
        || t.starts_with("<local-command-stdout>")
        || t.starts_with("<local-command-stderr>")
        || t.starts_with("<local-command-caveat>")
    {
        return None;
    }
    if let Some(name) = between(t, "<command-name>", "</command-name>") {
        let args = between(t, "<command-args>", "</command-args>").unwrap_or_default();
        let line = format!("{} {}", name.trim(), args.trim());
        return Some(line.trim().to_owned());
    }
    Some(t.to_owned())
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

/// Where a transcript's subagents are kept: `<folder>/<id>/subagents/`.
pub fn subagents_dir(transcript: &Path) -> PathBuf {
    transcript.with_extension("").join("subagents")
}

/// The newest write to a transcript or to one of its subagents' files.
pub fn last_write(transcript: &Path) -> Option<SystemTime> {
    let own = std::fs::metadata(transcript).and_then(|m| m.modified()).ok();
    let subs = std::fs::read_dir(subagents_dir(transcript))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max();
    own.max(subs)
}

/// The text a `content_block_delta` stream event adds to the message being written, if any.
pub fn text_delta(v: &Value) -> Option<&str> {
    if str_at(v, "/type") != "stream_event" || v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
        return None;
    }
    let event = v.get("event")?;
    if str_at(event, "/type") != "content_block_delta" || str_at(event, "/delta/type") != "text_delta" {
        return None;
    }
    event.pointer("/delta/text").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn log(lines: &[Value]) -> SessionLog {
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        SessionLog::from_transcript(&text.join("\n"))
    }

    fn kinds(log: &SessionLog) -> Vec<(ItemKind, String)> {
        log.items().iter().map(|i| (i.kind, i.text.clone())).collect()
    }

    #[test]
    fn reads_a_transcript_with_tools_and_markers() {
        let l = log(&[
            json!({"type":"queue-operation","operation":"enqueue"}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"Fix the bug"}]}}),
            json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}),
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"Looking."}]}}),
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm test"}}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]}}),
            json!({"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}),
            json!({"type":"system","subtype":"compact_boundary"}),
            json!({"type":"user","message":{"content":"<command-name>/review-local</command-name>\n<command-args>--base main</command-args>"}}),
            json!({"type":"ai-title","aiTitle":"Fix the failing test"}),
        ]);
        let k = kinds(&l);
        assert_eq!(k[0], (ItemKind::User, "Fix the bug".into()));
        assert_eq!(k[1], (ItemKind::Text, "Looking.".into()));
        assert_eq!(l.items()[2].tool.as_deref(), Some("Bash"));
        assert_eq!(l.items()[2].output.as_deref(), Some("ok"));
        assert_eq!(l.items()[3].tool.as_deref(), Some(END_INTERRUPTED));
        assert_eq!(l.items()[4].tool.as_deref(), Some(END_COMPACTED));
        assert_eq!(k[5], (ItemKind::User, "/review-local --base main".into()));
        assert_eq!(k.len(), 6);
        assert_eq!(l.title.as_deref(), Some("Fix the failing test"));
    }

    #[test]
    fn live_lines_mark_what_changed_and_skip_echoes() {
        let mut l = log(&[json!({"type":"user","message":{"content":"hi"}})]);
        assert_eq!(l.take_changed(), None);
        l.apply(&json!({"type":"user","message":{"content":[{"type":"text","text":"hi"}]}}));
        assert_eq!(l.items().len(), 1, "a replayed message seen twice shows once");
        l.apply(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Read","input":{"file_path":"x.rs"}}]}}));
        assert_eq!(l.take_changed(), Some(1));
        l.apply(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","content":"fn main"}]}}));
        assert_eq!(l.take_changed(), Some(1), "the tool call got its result");
        l.apply(&json!({"type":"result","subtype":"error_during_execution","is_error":true}));
        l.apply(&json!({"type":"user","message":{"content":"[Request interrupted by user]"}}));
        assert_eq!(l.items().iter().filter(|i| i.kind == ItemKind::End).count(), 1);
        l.apply(&json!({"type":"result","subtype":"success","is_error":false,"result":"done"}));
        assert_eq!(l.items().len(), 3, "a successful turn adds nothing");
    }

    /// The shape of CLI 2.1.329 stream-json for an Agent call (it went to the background).
    #[test]
    fn a_background_subagent_shows_under_its_call() {
        let mut l = SessionLog::default();
        l.apply(&json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[
            {"type":"tool_use","id":"A1","name":"Agent","input":{"description":"Count files","prompt":"Glob *.md"}}]}}));
        l.apply(&json!({"type":"system","subtype":"task_started","task_id":"k1","tool_use_id":"A1"}));
        l.apply(&json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result",
            "tool_use_id":"A1","content":[{"type":"text","text":"Async agent launched successfully. (internal) agentId: k1"}]}]}}));
        let agent = &l.items()[0];
        assert!(agent.running && agent.output.is_none(), "{agent:?}");
        l.take_changed();
        l.apply(
            &json!({"type":"assistant","parent_tool_use_id":"A1","message":{"content":[
            {"type":"tool_use","id":"g1","name":"Glob","input":{"pattern":"*.md"}}]}}),
        );
        l.apply(
            &json!({"type":"user","parent_tool_use_id":"A1","message":{"content":[
            {"type":"tool_result","tool_use_id":"g1","content":"No files found"}]}}),
        );
        l.apply(&json!({"type":"assistant","parent_tool_use_id":"A1","message":{"content":[{"type":"text","text":"0"}]}}));
        assert_eq!(l.take_changed(), Some(0), "the call changed, nothing was added");
        assert_eq!(l.items().len(), 1);
        let children = &l.items()[0].children;
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].tool.as_deref(), Some("Glob"));
        assert_eq!(children[0].output.as_deref(), Some("No files found"));
        assert_eq!(children[1].text, "0");
        l.apply(
            &json!({"type":"system","subtype":"task_notification","tool_use_id":"A1","status":"completed",
            "summary":"0","summary_is_report":true}),
        );
        let agent = &l.items()[0];
        assert!(!agent.running && !agent.failed);
        assert_eq!(agent.output.as_deref(), Some("0"));
    }

    #[test]
    fn a_task_notification_in_the_transcript_is_not_a_user_message() {
        let note = "<task-notification>\n<task-id>k1</task-id>\n<tool-use-id>A1</tool-use-id>\n\
            <status>failed</status>\n<summary>Agent \"d\" failed</summary>\n<result>boom</result>\n</task-notification>";
        let l = log(&[
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"A1","name":"Agent","input":{"description":"d"}}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"A1","content":"Async agent launched successfully."}]}}),
            json!({"type":"user","message":{"role":"user","content":note}}),
        ]);
        assert_eq!(l.items().len(), 1);
        let agent = &l.items()[0];
        assert!(!agent.running && agent.failed);
        assert_eq!(agent.output.as_deref(), Some("boom"));
    }

    #[test]
    fn a_transcript_reads_its_subagents_files() {
        let dir = std::env::temp_dir().join(format!("opw-sublog-{}", std::process::id()));
        let subs = dir.join("s1").join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let lines = |v: &[Value]| v.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
        let main = dir.join("s1.jsonl");
        std::fs::write(
            &main,
            lines(&[
                json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"A1","name":"Agent","input":{"description":"d"}}]}}),
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"A1","content":"Async agent launched successfully."}]}}),
            ]),
        )
        .unwrap();
        std::fs::write(
            subs.join("agent-x.meta.json"),
            r#"{"toolUseId":"A1","finished":false}"#,
        )
        .unwrap();
        std::fs::write(
            subs.join("agent-x.jsonl"),
            lines(&[
                json!({"type":"fork-context-ref","agentId":"x"}),
                json!({"type":"user","isSidechain":true,"message":{"content":"Glob *.md"}}),
                json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"g1","name":"Glob","input":{"pattern":"*.md"}}]}}),
            ]),
        )
        .unwrap();
        let l = SessionLog::read_file(&main).unwrap();
        let agent = &l.items()[0];
        assert!(agent.running);
        assert_eq!(agent.children.len(), 1, "the prompt is on the call, not repeated");
        assert_eq!(agent.children[0].tool.as_deref(), Some("Glob"));
        assert!(last_write(&main).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `OPW_TRANSCRIPT=<path to a .jsonl> cargo test -p opw-agents -- --ignored real_transcript`
    #[test]
    #[ignore = "reads a transcript on this machine"]
    fn real_transcript_with_subagents() {
        let path = std::env::var("OPW_TRANSCRIPT").expect("OPW_TRANSCRIPT");
        let l = SessionLog::read_file(Path::new(&path)).unwrap();
        for i in l.items().iter().filter(|i| !i.children.is_empty() || i.running) {
            println!(
                "{:?} {} running={} children={} output={:?}",
                i.tool,
                cut(&i.text, 60),
                i.running,
                i.children.len(),
                i.output.as_deref().map(|o| cut(o, 80))
            );
        }
        let users = l.items().iter().filter(|i| i.kind == ItemKind::User);
        assert!(users.clone().all(|i| !i.text.starts_with(TASK_NOTIFICATION)));
    }

    #[test]
    fn subagent_messages_and_deltas() {
        let mut l = SessionLog::default();
        // a subagent of a call this log does not have: nowhere to show it
        l.apply(&json!({"type":"assistant","parent_tool_use_id":"t9","message":{"content":[{"type":"text","text":"inner"}]}}));
        assert!(l.items().is_empty());
        let delta = json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}});
        assert_eq!(text_delta(&delta), Some("Hel"));
        let thinking = json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"…"}}});
        assert_eq!(text_delta(&thinking), None);
    }

    #[test]
    fn old_items_go_in_steps() {
        let mut l = SessionLog::default();
        for i in 0..ITEMS_MAX {
            l.apply(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":format!("t{i}"),"name":"Read","input":{}}]}}));
        }
        l.take_changed();
        l.apply(&json!({"type":"assistant","message":{"content":[{"type":"text","text":"more"}]}}));
        assert_eq!(l.dropped(), DROP_STEP);
        assert_eq!(l.items().len(), ITEMS_MAX - DROP_STEP + 1);
        assert_eq!(l.take_changed(), Some(0));
        // ids still point at the right items after the shift
        l.apply(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":format!("t{}", ITEMS_MAX - 1),"content":"x"}]}}));
        assert_eq!(l.items()[ITEMS_MAX - DROP_STEP - 1].output.as_deref(), Some("x"));
    }
}
