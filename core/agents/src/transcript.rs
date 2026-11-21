//! A step's raw output (`<data>/runs/<task>/<stamp>-<step>.jsonl`, one JSON object per line)
//! turned into the items of the task window's Chat tab: the prompt the app sent, what the AI
//! wrote, each tool call with a shortened result, and calls the CLI refused. Reads Claude
//! stream-json and Codex `exec --json`; unknown lines are skipped, so a newer CLI that adds
//! event types still shows what this version understands.

use std::collections::HashMap;

use serde_json::Value;

use crate::claude::{cut, tool_input};

/// Type of the line the app writes first in every run log: the prompt it sent.
pub const PROMPT_TYPE: &str = "opw.prompt";

/// Longest prompt kept (the brief can be long).
pub const PROMPT_MAX: usize = 8000;
/// Longest AI message kept.
pub const TEXT_MAX: usize = 20_000;
/// Longest tool input kept (a command, a path, a todo list).
pub const INPUT_MAX: usize = 2000;
/// Longest tool result kept; the rest is counted in [`Item::output_cut`].
pub const OUTPUT_MAX: usize = 1500;
/// Items kept; older ones (after the prompt) are dropped first.
pub const ITEMS_MAX: usize = 400;

/// The first line of a run log: `{"type":"opw.prompt","text":…}`.
pub fn prompt_line(text: &str) -> String {
    serde_json::json!({ "type": PROMPT_TYPE, "text": cut(text, PROMPT_MAX) }).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// What the app sent (stdin, or the slash command and its added instructions).
    Prompt,
    /// A message the user typed in a session (Workspace chat).
    User,
    /// A message the AI wrote (markdown).
    Text,
    /// Reasoning the CLI showed; Claude often hides it (empty text), then nothing is kept.
    Thinking,
    Tool,
    /// The run ended; `tool` holds Claude's result subtype (`success`, `error_max_turns`, …).
    End,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    pub message_id: Option<String>,
    /// The prompt, the message, the tool's input (command, path) or the error.
    pub text: String,
    pub tool: Option<String>,
    /// What the tool returned, at most [`OUTPUT_MAX`] characters.
    pub output: Option<String>,
    /// Characters of the tool result left out.
    pub output_cut: usize,
    /// The tool failed (error result, exit code not 0) or the run ended with an error.
    pub failed: bool,
    /// Why the CLI refused the call (not on the allow list, …).
    pub denied: Option<String>,
    /// What a subagent did under this Agent / Task call (its tool calls and messages).
    pub children: Vec<Item>,
    /// A subagent of this call still runs (in the background: the call itself already returned).
    pub running: bool,
    /// The tool call's id (`tool_use` id), for a tool item.
    pub tool_id: Option<String>,
}

impl Item {
    pub(crate) fn new(kind: ItemKind, text: String) -> Self {
        Self {
            kind,
            message_id: None,
            text,
            tool: None,
            output: None,
            output_cut: 0,
            failed: false,
            denied: None,
            children: Vec::new(),
            running: false,
            tool_id: None,
        }
    }

    pub(crate) fn tool(name: &str, text: String) -> Self {
        Self {
            tool: Some(name.to_owned()),
            ..Self::new(ItemKind::Tool, text)
        }
    }

    pub(crate) fn set_output(&mut self, text: &str) {
        let text = text.trim_end();
        let total = text.chars().count();
        if total > OUTPUT_MAX {
            self.output = Some(text.chars().take(OUTPUT_MAX).collect());
            self.output_cut = total - OUTPUT_MAX;
        } else {
            self.output = Some(text.to_owned());
            self.output_cut = 0;
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    pub items: Vec<Item>,
    /// Older items left out to stay under [`ITEMS_MAX`].
    pub dropped: usize,
}

#[derive(Default)]
struct Builder {
    items: Vec<Item>,
    /// tool_use_id / Codex item id → index in `items`
    by_id: HashMap<String, usize>,
}

impl Builder {
    fn push(&mut self, item: Item) -> usize {
        self.items.push(item);
        self.items.len() - 1
    }

    fn push_with_id(&mut self, id: &str, item: Item) {
        let at = self.push(item);
        if !id.is_empty() {
            self.by_id.insert(id.to_owned(), at);
        }
    }

    fn get(&mut self, id: &str) -> Option<&mut Item> {
        let at = *self.by_id.get(id)?;
        self.items.get_mut(at)
    }

    fn finish(self) -> Transcript {
        let mut items = self.items;
        let mut dropped = 0;
        if items.len() > ITEMS_MAX {
            let keep_first = usize::from(items.first().is_some_and(|i| i.kind == ItemKind::Prompt));
            dropped = items.len() - ITEMS_MAX;
            items.drain(keep_first..keep_first + dropped);
        }
        Transcript { items, dropped }
    }
}

pub(crate) fn str_at<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or_default()
}

/// Read a run log. `codex` picks the Codex format; the prompt line works with both.
pub fn read(log: &str, codex: bool) -> Transcript {
    let mut b = Builder::default();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if str_at(&v, "/type") == PROMPT_TYPE {
            let text = str_at(&v, "/text").trim();
            if !text.is_empty() {
                b.push(Item::new(ItemKind::Prompt, text.to_owned()));
            }
        } else if codex {
            codex_line(&mut b, &v);
        } else {
            claude_line(&mut b, &v);
        }
    }
    b.finish()
}

// ---------- Claude stream-json ----------

fn claude_line(b: &mut Builder, v: &Value) {
    match str_at(v, "/type") {
        "assistant" => {
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
                            b.push(Item::new(ItemKind::Text, cut(text, TEXT_MAX)));
                        }
                    }
                    "thinking" => {
                        let text = str_at(block, "/thinking").trim();
                        if !text.is_empty() {
                            b.push(Item::new(ItemKind::Thinking, cut(text, TEXT_MAX)));
                        }
                    }
                    "tool_use" => {
                        let name = str_at(block, "/name");
                        let input = block.get("input").unwrap_or(&Value::Null);
                        b.push_with_id(
                            str_at(block, "/id"),
                            Item::tool(name, claude_tool_text(name, input)),
                        );
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            for block in v
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if str_at(block, "/type") != "tool_result" {
                    continue;
                }
                let text = result_text(block.get("content").unwrap_or(&Value::Null));
                let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                if let Some(item) = b.get(str_at(block, "/tool_use_id")) {
                    item.set_output(&text);
                    item.failed |= failed;
                }
            }
        }
        "system" if str_at(v, "/subtype") == "permission_denied" => {
            let reason = [str_at(v, "/decision_reason"), str_at(v, "/message")]
                .into_iter()
                .map(str::trim)
                .find(|s| !s.is_empty())
                .unwrap_or("bị từ chối");
            let reason = cut(reason, 300);
            if let Some(item) = b.get(str_at(v, "/tool_use_id")) {
                item.denied = Some(reason);
                item.failed = true;
            }
        }
        "result" => {
            let failed = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let mut end = Item::new(
                ItemKind::End,
                if failed {
                    cut(str_at(v, "/result").trim(), TEXT_MAX)
                } else {
                    String::new()
                },
            );
            end.tool = Some(str_at(v, "/subtype").to_owned()).filter(|s| !s.is_empty());
            end.failed = failed;
            b.push(end);
        }
        _ => {}
    }
}

/// What a tool call works on, whole: the command, the path, `pattern in path`, the todo list.
pub fn claude_tool_text(name: &str, input: &Value) -> String {
    match name {
        "Grep" => {
            let pattern = str_at(input, "/pattern");
            match str_at(input, "/path") {
                "" => pattern.to_owned(),
                path => format!("{pattern}  ·  {path}"),
            }
        }
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|todos| {
                let lines: Vec<String> = todos
                    .iter()
                    .map(|t| {
                        let mark = match str_at(t, "/status") {
                            "completed" => "[x]",
                            "in_progress" => "[~]",
                            _ => "[ ]",
                        };
                        format!("{mark} {}", str_at(t, "/content"))
                    })
                    .collect();
                cut(&lines.join("\n"), INPUT_MAX)
            })
            .unwrap_or_default(),
        _ => {
            let text = tool_input(input, INPUT_MAX);
            if !text.is_empty() {
                return text;
            }
            for key in ["query", "skill", "subagent_type", "prompt"] {
                let s = str_at(input, &format!("/{key}")).trim();
                if !s.is_empty() {
                    return cut(s, INPUT_MAX);
                }
            }
            match input {
                Value::Null => String::new(),
                Value::Object(m) if m.is_empty() => String::new(),
                other => cut(&other.to_string(), 500),
            }
        }
    }
}

/// `content` of a tool_result: a string, or blocks whose `text` parts are joined.
pub(crate) fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ---------- Codex exec --json ----------

fn codex_line(b: &mut Builder, v: &Value) {
    let kind = str_at(v, "/type");
    let item = v.get("item").unwrap_or(&Value::Null);
    let id = str_at(item, "/id");
    match (kind, str_at(item, "/type")) {
        ("item.started", "command_execution") => {
            b.push_with_id(id, Item::tool("Shell", unwrap_shell(str_at(item, "/command"))));
        }
        ("item.completed", "command_execution") => {
            let output = str_at(item, "/aggregated_output").to_owned();
            let failed = item
                .get("exit_code")
                .and_then(Value::as_i64)
                .is_some_and(|c| c != 0)
                || str_at(item, "/status") == "failed"
                || str_at(item, "/status") == "declined";
            if b.get(id).is_none() {
                b.push_with_id(id, Item::tool("Shell", unwrap_shell(str_at(item, "/command"))));
            }
            if let Some(it) = b.get(id) {
                it.set_output(&output);
                it.failed |= failed;
                if str_at(item, "/status") == "declined" {
                    it.denied = Some("Codex không được phép chạy lệnh này".into());
                }
            }
        }
        ("item.completed", "agent_message") => {
            let text = str_at(item, "/text").trim();
            if !text.is_empty() {
                b.push(Item::new(ItemKind::Text, cut(text, TEXT_MAX)));
            }
        }
        ("item.completed", "reasoning") => {
            let text = str_at(item, "/text").trim();
            if !text.is_empty() {
                b.push(Item::new(ItemKind::Thinking, cut(text, TEXT_MAX)));
            }
        }
        ("item.completed", "file_change") => {
            let files: Vec<String> = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|c| match str_at(c, "/kind") {
                    "" => str_at(c, "/path").to_owned(),
                    k => format!("{} ({k})", str_at(c, "/path")),
                })
                .collect();
            let mut it = Item::tool("Edit", cut(&files.join("\n"), INPUT_MAX));
            it.failed = str_at(item, "/status") == "failed";
            b.push(it);
        }
        ("item.completed", "mcp_tool_call") => {
            let name = format!("{}.{}", str_at(item, "/server"), str_at(item, "/tool"));
            let mut it = Item::tool(&name, String::new());
            it.failed = str_at(item, "/status") == "failed";
            b.push(it);
        }
        ("item.completed", "web_search") => {
            b.push(Item::tool("WebSearch", str_at(item, "/query").to_owned()));
        }
        ("item.completed", "todo_list") => {
            let lines: Vec<String> = item
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|t| {
                    let done = t.get("completed").and_then(Value::as_bool).unwrap_or(false);
                    format!("{} {}", if done { "[x]" } else { "[ ]" }, str_at(t, "/text"))
                })
                .collect();
            b.push(Item::tool("Todo", cut(&lines.join("\n"), INPUT_MAX)));
        }
        ("item.completed", "error") => {
            let mut it = Item::new(ItemKind::Error, str_at(item, "/message").to_owned());
            it.failed = true;
            b.push(it);
        }
        ("turn.failed", _) | ("error", _) => {
            let message = [str_at(v, "/error/message"), str_at(v, "/message")]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or_default();
            let mut it = Item::new(ItemKind::Error, cut(message, TEXT_MAX));
            it.failed = true;
            b.push(it);
        }
        ("turn.completed", _) => {
            b.push(Item::new(ItemKind::End, String::new()));
        }
        _ => {}
    }
}

/// Codex on Windows wraps each command as `"…\powershell.exe" -Command '<cmd>'` (and `bash -lc
/// '<cmd>'` elsewhere): show only `<cmd>`.
fn unwrap_shell(command: &str) -> String {
    let c = command.trim();
    // ASCII-only lowering keeps byte offsets valid for slicing `c`
    let lower = c.to_ascii_lowercase();
    let inner = ["-command ", "-lc "].iter().find_map(|flag| {
        let at = lower.find(flag)?;
        let head = &lower[..at];
        let is_shell = ["powershell", "pwsh", "bash", "zsh", "/sh", "\\sh"]
            .iter()
            .any(|s| head.contains(s));
        is_shell.then(|| &c[at + flag.len()..])
    });
    let Some(inner) = inner else {
        return cut(c, INPUT_MAX);
    };
    let inner = inner.trim();
    let unquoted = ['\'', '"']
        .iter()
        .find_map(|q| inner.strip_prefix(*q).and_then(|s| s.strip_suffix(*q)))
        .unwrap_or(inner);
    cut(unquoted, INPUT_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_log() -> String {
        [
            prompt_line("Implement the plan"),
            r#"{"type":"system","subtype":"init","session_id":"s-1","model":"m"}"#.into(),
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"","signature":"x"},{"type":"text","text":"Đọc file trước."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"src/a.ts"}}]}}"#.into(),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"line 1\nline 2"}]}}"#.into(),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cd api && npx tsc --noEmit","description":"Type check"}}]}}"#.into(),
            r#"{"type":"system","subtype":"permission_denied","tool_name":"Bash","tool_use_id":"t2","decision_reason":"not in the allow list","message":"long text"}"#.into(),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":[{"type":"text","text":"Permission denied"}]}]}}"#.into(),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"Grep","input":{"pattern":"orgId","path":"api/src"}},{"type":"tool_use","id":"t4","name":"TodoWrite","input":{"todos":[{"content":"Sửa route","status":"completed"},{"content":"Chạy test","status":"pending"}]}}]}}"#.into(),
            "not json".into(),
            r#"{"type":"rate_limit_event"}"#.into(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Xong: sửa 1 file."}]}}"#.into(),
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Xong: sửa 1 file."}"#.into(),
        ]
        .join("\n")
    }

    #[test]
    fn claude_log_becomes_chat_items() {
        let t = read(&claude_log(), false);
        let kinds: Vec<ItemKind> = t.items.iter().map(|i| i.kind).collect();
        use ItemKind::*;
        assert_eq!(kinds, [Prompt, Text, Tool, Tool, Tool, Tool, Text, End]);
        let read_file = &t.items[2];
        assert_eq!(read_file.tool.as_deref(), Some("Read"));
        assert_eq!(read_file.text, "src/a.ts");
        assert_eq!(read_file.output.as_deref(), Some("line 1\nline 2"));
        let bash = &t.items[3];
        assert_eq!(bash.text, "cd api && npx tsc --noEmit");
        assert_eq!(bash.denied.as_deref(), Some("not in the allow list"));
        assert!(bash.failed);
        assert_eq!(bash.output.as_deref(), Some("Permission denied"));
        assert_eq!(t.items[4].text, "orgId  ·  api/src");
        assert_eq!(t.items[5].text, "[x] Sửa route\n[ ] Chạy test");
        assert_eq!(t.items[7].tool.as_deref(), Some("success"));
        assert!(!t.items[7].failed && t.items[7].text.is_empty());
        assert_eq!(t.dropped, 0);
    }

    #[test]
    fn long_results_are_cut_and_old_items_dropped() {
        let long = "x".repeat(OUTPUT_MAX + 25);
        let mut lines = vec![prompt_line("p")];
        for i in 0..ITEMS_MAX + 5 {
            lines.push(format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t{i}","name":"Read","input":{{"file_path":"f{i}"}}}}]}}}}"#
            ));
        }
        lines.push(format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t{}","content":"{long}"}}]}}}}"#,
            ITEMS_MAX + 4
        ));
        let t = read(&lines.join("\n"), false);
        assert_eq!(t.items.len(), ITEMS_MAX);
        assert_eq!(t.dropped, 6);
        assert_eq!(t.items[0].kind, ItemKind::Prompt);
        let last = t.items.last().unwrap();
        assert_eq!(last.text, format!("f{}", ITEMS_MAX + 4));
        assert_eq!(last.output.as_ref().map(|o| o.chars().count()), Some(OUTPUT_MAX));
        assert_eq!(last.output_cut, 25);
    }

    #[test]
    fn codex_log_becomes_chat_items() {
        let log = [
            prompt_line("Review the change"),
            r#"{"type":"thread.started","thread_id":"th-1"}"#.into(),
            r#"{"type":"item.completed","item":{"id":"i0","type":"reasoning","text":"Xem diff trước"}}"#.into(),
            r#"{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"\"C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -Command 'git diff origin/main'","status":"in_progress"}}"#.into(),
            r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"x","aggregated_output":"diff --git a b","exit_code":0,"status":"completed"}}"#.into(),
            r#"{"type":"item.completed","item":{"id":"i2","type":"command_execution","command":"bash -lc \"npm test\"","aggregated_output":"1 failed","exit_code":1,"status":"failed"}}"#.into(),
            r#"{"type":"item.completed","item":{"id":"i3","type":"file_change","changes":[{"path":"src/a.ts","kind":"update"}],"status":"completed"}}"#.into(),
            r#"{"type":"item.completed","item":{"id":"i4","type":"agent_message","text":"{\"findings\":[]}"}}"#.into(),
            r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#.into(),
        ]
        .join("\n");
        let t = read(&log, true);
        use ItemKind::*;
        let kinds: Vec<ItemKind> = t.items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [Prompt, Thinking, Tool, Tool, Tool, Text, End]);
        assert_eq!(t.items[2].text, "git diff origin/main");
        assert_eq!(t.items[2].output.as_deref(), Some("diff --git a b"));
        assert!(!t.items[2].failed);
        assert_eq!(t.items[3].text, "npm test");
        assert!(t.items[3].failed);
        assert_eq!(t.items[4].tool.as_deref(), Some("Edit"));
        assert_eq!(t.items[4].text, "src/a.ts (update)");
    }

    #[test]
    fn codex_errors_and_plain_commands() {
        let log = [
            r#"{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"git status","status":"in_progress"}}"#,
            r#"{"type":"turn.failed","error":{"message":"401 Unauthorized"}}"#,
        ]
        .join("\n");
        let t = read(&log, true);
        assert_eq!(t.items[0].text, "git status");
        assert_eq!(t.items[1].kind, ItemKind::Error);
        assert_eq!(t.items[1].text, "401 Unauthorized");
        assert!(read("", true).items.is_empty());
    }
}
