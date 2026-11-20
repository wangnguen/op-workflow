//! The conversation of one Claude Code session as the Workspace chat shows it: what the user
//! typed, what the AI wrote, each tool call with a shortened result, interruptions and compacts.
//!
//! The same lines arrive from two places: the session's transcript
//! (`~/.claude/projects/<folder>/<id>.jsonl`, read when a session is opened) and the stream-json
//! output of the process the app runs for it (appended live). Both carry `user` / `assistant`
//! messages with the same `message.content` blocks, so one builder reads both. Unknown lines are
//! skipped, so a newer CLI that adds line types still shows what this version understands.

use std::collections::HashMap;

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
        // a subagent's own messages: its result shows on the Task / Agent tool call
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || v.get("parent_tool_use_id").is_some_and(|p| !p.is_null())
        {
            return;
        }
        match str_at(v, "/type") {
            "user" => self.user(v),
            "assistant" => self.assistant(v),
            "system" => match str_at(v, "/subtype") {
                "compact_boundary" => self.end(END_COMPACTED),
                "permission_denied" => self.denied(v),
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
        // the echo of a message already shown (a replayed message seen twice)
        if self
            .items
            .last()
            .is_some_and(|i| i.kind == ItemKind::User && i.text == text)
        {
            return;
        }
        self.push(Item::new(ItemKind::User, cut(&text, TEXT_MAX)));
    }

    fn tool_result(&mut self, block: &Value) {
        let text = result_text(block.get("content").unwrap_or(&Value::Null));
        let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        if let Some(&at) = self.by_id.get(str_at(block, "/tool_use_id")) {
            if let Some(item) = self.items.get_mut(at) {
                item.set_output(&text);
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
                    let at = self.push(Item::tool(name, claude_tool_text(name, input)));
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

    #[test]
    fn subagent_messages_and_deltas() {
        let mut l = SessionLog::default();
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
