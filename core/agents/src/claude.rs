//! Claude Code headless (`claude -p --output-format stream-json --verbose`).

use serde_json::Value;

use crate::{AgentsError, AgentsResult, ModelChoice};

/// git subcommands an AI process may never run (defence in depth on top of the env guard).
pub const DISALLOWED_GIT: &str = "Bash(git push *) Bash(git commit *) Bash(git merge *) Bash(git rebase *) \
                                  Bash(git reset *) Bash(git clean *) Bash(git tag *) Bash(gh pr *)";

/// Longest tool detail kept for the log (a file path, a pattern, the first line of a command).
/// Long enough for a real path or command; the UI wraps it instead of cutting it.
pub const TOOL_DETAIL_MAX: usize = 300;

/// Asked of every review the app reads (`/review-local` and the app's own prompt), on top of
/// the repo's command, so it costs no extra turn.
pub const REVIEW_LANGUAGE: &str = "Write the title, detail and suggestion of every finding in Vietnamese, \
     whatever language the rest of your answer uses. Keep code, identifiers, file paths, error \
     messages and quoted text exactly as they are. If you judge whether documentation or tests \
     need updating, put that judgement, in Vietnamese, in a `notes` string field next to \
     `findings` in the json block.";

/// The final `result` line of a headless run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunResult {
    /// `success`, `error_max_turns`, `error_during_execution`, `error_max_budget_usd`, ...
    pub subtype: String,
    pub is_error: bool,
    /// The last assistant message: the plan, the review with its JSON block, the summary.
    pub text: String,
    pub session_id: String,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub num_turns: u64,
    /// Every input token the model read (fresh + cache writes + cache reads).
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Tool calls refused because nobody could approve them (`--permission-prompts none`),
    /// e.g. `Bash: npx jest`.
    pub denials: Vec<String>,
}

impl RunResult {
    pub fn over_budget(&self) -> bool {
        self.subtype.contains("budget")
    }
}

/// Events the app cares about from one stream-json line.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeEvent {
    Init {
        session_id: String,
        model: String,
        tools: Vec<String>,
    },
    ToolUse {
        name: String,
        /// File, pattern or command the tool works on (shortened), may be empty.
        detail: String,
    },
    AssistantText,
    /// Tokens in the context after an assistant turn (input + cache reads + cache writes).
    Context(u64),
    /// Raw `rate_limit_event`; `opw-quota` turns it into a QuotaSnapshot.
    RateLimit(Value),
    Result(RunResult),
    Other(String),
}

fn u64_at(v: &Value, pointer: &str) -> u64 {
    v.pointer(pointer).and_then(Value::as_u64).unwrap_or_default()
}

/// Input tokens of a `usage` object: fresh + cache writes + cache reads.
fn input_tokens(usage: &Value) -> u64 {
    u64_at(usage, "/input_tokens")
        + u64_at(usage, "/cache_creation_input_tokens")
        + u64_at(usage, "/cache_read_input_tokens")
}

/// Shorten to `max` characters on a char boundary, first line only.
pub fn short(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= max {
        line.to_owned()
    } else {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn tool_detail(input: &Value) -> String {
    ["file_path", "path", "pattern", "command", "url", "description"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .map(|s| short(s, TOOL_DETAIL_MAX))
        .unwrap_or_default()
}

pub fn parse_line(line: &str) -> AgentsResult<Vec<ClaudeEvent>> {
    let v: Value = serde_json::from_str(line).map_err(|e| AgentsError::BadLine(e.to_string()))?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
    let s = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    Ok(match kind {
        "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => vec![ClaudeEvent::Init {
            session_id: s("session_id"),
            model: s("model"),
            tools: v
                .get("tools")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
        }],
        "assistant" => {
            let blocks = v
                .pointer("/message/content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut events: Vec<ClaudeEvent> = blocks
                .iter()
                .map(|b| match b.get("type").and_then(Value::as_str) {
                    Some("tool_use") => ClaudeEvent::ToolUse {
                        name: b
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        detail: b.get("input").map(tool_detail).unwrap_or_default(),
                    },
                    _ => ClaudeEvent::AssistantText,
                })
                .collect();
            if let Some(usage) = v.pointer("/message/usage") {
                let context = input_tokens(usage);
                if context > 0 {
                    events.push(ClaudeEvent::Context(context));
                }
            }
            events
        }
        "rate_limit_event" => vec![ClaudeEvent::RateLimit(v)],
        "result" => {
            let usage = v.get("usage").cloned().unwrap_or(Value::Null);
            let denials = v
                .get("permission_denials")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|d| {
                            let tool = d.get("tool_name").and_then(Value::as_str).unwrap_or("?");
                            match d.get("tool_input").map(tool_detail) {
                                Some(detail) if !detail.is_empty() => format!("{tool}: {detail}"),
                                _ => tool.to_owned(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            vec![ClaudeEvent::Result(RunResult {
                subtype: s("subtype"),
                is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                text: s("result"),
                session_id: s("session_id"),
                cost_usd: v
                    .get("total_cost_usd")
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or_default(),
                num_turns: v.get("num_turns").and_then(Value::as_u64).unwrap_or_default(),
                tokens_in: input_tokens(&usage),
                tokens_out: u64_at(&usage, "/output_tokens"),
                denials,
            })]
        }
        other => vec![ClaudeEvent::Other(other.to_owned())],
    })
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

/// Ask the CLI which models and effort levels this account can use, without any prompt (no
/// API call, no tokens): start a stream-json session, send [`INITIALIZE_REQUEST`] on stdin, read
/// the `control_response` line (`response.response.models`), then close stdin. Checked with
/// Claude Code 2.1.325. Settings and plugins stay off so no hook runs; the model list does not
/// depend on them (the machine's default model is read from settings.json separately).
pub fn catalog_args() -> Vec<String> {
    strings(&[
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--no-session-persistence",
    ])
}

/// The control request [`catalog_args`] answers with the model list.
pub const INITIALIZE_REQUEST: &str =
    r#"{"type":"control_request","request_id":"opw-models","request":{"subtype":"initialize"}}"#;

/// Minimal quota probe (PLAN.md §9): ~645 input + ~90 output Haiku tokens, run in an empty
/// directory so no CLAUDE.md is loaded. Read the `rate_limit_event` line from its output.
pub fn quota_probe_args(model: &str) -> Vec<String> {
    strings(&[
        "-p",
        ".",
        "--model",
        model,
        "--system-prompt",
        "Reply with one dot.",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--max-turns",
        "1",
        "--output-format",
        "stream-json",
        "--verbose",
    ])
}

/// Read-only git a reviewer may run without asking.
pub const REVIEW_BASH: &str =
    "Bash(git diff *) Bash(git status *) Bash(git log *) Bash(git show *) Bash(git merge-base *)";

/// Tools of a code-writing step (implement, fix).
const CODE_TOOLS: &str = "Read,Grep,Glob,Edit,Write,Bash";

fn budget(usd: f64) -> String {
    format!("{usd}")
}

fn stream_json(mut args: Vec<String>) -> Vec<String> {
    args.extend(strings(&["--output-format", "stream-json", "--verbose"]));
    args
}

/// Add `--model` / `--effort` when the user picked them for this step (before the output flags).
pub fn with_model(mut args: Vec<String>, choice: &ModelChoice) -> Vec<String> {
    let at = args
        .iter()
        .position(|a| a == "--output-format")
        .unwrap_or(args.len());
    let mut flags = Vec::new();
    if let Some(m) = &choice.model {
        flags.extend(["--model".to_owned(), m.clone()]);
    }
    if let Some(e) = &choice.effort {
        flags.extend(["--effort".to_owned(), e.clone()]);
    }
    args.splice(at..at, flags);
    args
}

/// [2] Analyze: read only. `--restricted` also drops Bash/WebFetch and the user's settings
/// files; the prompt (brief + plan template) comes on stdin.
pub fn analyze_args(budget_usd: f64) -> Vec<String> {
    stream_json(strings(&[
        "-p",
        "--restricted",
        "--strict-mcp-config",
        "--tools",
        "Read,Grep,Glob",
        "--permission-prompts",
        "none",
        "--max-budget-usd",
        &budget(budget_usd),
    ]))
}

/// `npm run lint *` → `Bash(npm run lint *)`; entries already written as `Bash(...)` stay.
fn bash_rule(entry: &str) -> String {
    let e = entry.trim();
    if e.starts_with("Bash(") {
        e.to_owned()
    } else {
        format!("Bash({e})")
    }
}

/// [3] Implement and [5] Fix, headless. `resume` continues the analyze session so the AI keeps
/// the plan and what it read. Edits are accepted; Bash runs only what `allowed_bash` (plus the
/// repo's own allow list) permits, anything else is refused without asking. Git writes are
/// refused by name here and by the env guard.
pub fn code_args(resume: Option<&str>, budget_usd: f64, allowed_bash: &[String]) -> Vec<String> {
    let mut args = strings(&["-p"]);
    if let Some(id) = resume.filter(|id| !id.is_empty()) {
        args.extend(strings(&["--resume", id]));
    }
    args.extend(strings(&[
        "--strict-mcp-config",
        "--tools",
        CODE_TOOLS,
        "--permission-mode",
        "acceptEdits",
    ]));
    let allowed: Vec<String> = allowed_bash
        .iter()
        .filter(|e| !e.trim().is_empty())
        .map(|e| bash_rule(e))
        .collect();
    if !allowed.is_empty() {
        args.push("--allowedTools".into());
        args.push(allowed.join(" "));
    }
    args.extend(strings(&[
        "--permission-prompts",
        "none",
        "--disallowedTools",
        DISALLOWED_GIT,
        "--max-budget-usd",
        &budget(budget_usd),
    ]));
    stream_json(args)
}

/// `/review-local` run headless. The repo command hard-codes `development`, so the real base
/// of the task is passed in; `Skill` is blocked so it cannot chain into `/push-summary`.
pub fn review_local_args(base: &str, budget_usd: f64) -> Vec<String> {
    let prompt = format!(
        "Non-interactive run. The base branch for this task is {base}: wherever the command uses \
         development, use {base} instead. The changes are not committed yet: review the working \
         tree against {base} (git diff {base}, plus untracked files from git status). End with \
         one json block: {{findings:[{{severity,file,line,title,detail,suggestion}}], notes}}\n\
         {REVIEW_LANGUAGE}"
    );
    let mut args = strings(&[
        "-p",
        "/review-local",
        "--strict-mcp-config",
        "--tools",
        "Read,Grep,Glob,Bash",
        "--permission-prompts",
        "none",
        "--disallowedTools",
        "Skill",
        "--allowedTools",
        REVIEW_BASH,
        "--max-budget-usd",
        &budget(budget_usd),
        "--append-system-prompt",
    ]);
    args.push(prompt);
    stream_json(args)
}

/// The app's own review (repo without `.claude/commands/review-local.md`); prompt on stdin.
pub fn review_args(budget_usd: f64) -> Vec<String> {
    stream_json(strings(&[
        "-p",
        "--strict-mcp-config",
        "--tools",
        "Read,Grep,Glob,Bash",
        "--permission-prompts",
        "none",
        "--disallowedTools",
        "Skill",
        "--allowedTools",
        REVIEW_BASH,
        "--max-budget-usd",
        &budget(budget_usd),
    ]))
}

fn issue_note(issue: Option<u64>) -> String {
    match issue {
        Some(n) => format!("Issue number: #{n} (already provided, do not ask)"),
        None => "There is no issue number for this change (do not ask)".to_owned(),
    }
}

/// Effort of the push summary when the user has not picked one (what the app always used).
pub const PUSH_SUMMARY_EFFORT: &str = "low";

/// Flags shared by both push-summary forms: no session kept, git read only. The effort comes
/// from [`with_model`] (the user's pick, else [`PUSH_SUMMARY_EFFORT`]).
fn push_flags(budget_usd: f64) -> Vec<String> {
    strings(&[
        "--strict-mcp-config",
        "--no-session-persistence",
        // Without --tools the child loads every tool on the machine (~39k extra tokens, §9).
        "--tools",
        "Bash",
        "--permission-prompts",
        "none",
        "--disallowedTools",
        "Skill",
        "--allowedTools",
        "Bash(git diff *) Bash(git log *) Bash(git status *)",
        "--max-budget-usd",
        &budget(budget_usd),
    ])
}

/// `/push-summary` with the issue number the user confirmed on the push screen.
pub fn push_summary_args(issue: Option<u64>, budget_usd: f64) -> Vec<String> {
    let mut args = strings(&["-p", "/push-summary"]);
    args.extend(push_flags(budget_usd));
    args.push("--append-system-prompt".into());
    args.push(issue_note(issue));
    stream_json(args)
}

/// The app's own push summary (repo without the command); prompt on stdin.
pub fn push_summary_default_args(budget_usd: f64) -> Vec<String> {
    let mut args = strings(&["-p"]);
    args.extend(push_flags(budget_usd));
    stream_json(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shapes captured from a real `claude -p` run on 2026-11-13 (content removed).
    #[test]
    fn parses_stream_json_lines() {
        let init =
            r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-haiku","tools":[]}"#;
        assert_eq!(
            parse_line(init).unwrap(),
            vec![ClaudeEvent::Init {
                session_id: "s-1".into(),
                model: "claude-haiku".into(),
                tools: vec![]
            }]
        );
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"Read","input":{"file_path":"src/a.ts"}}],"usage":{"input_tokens":5,"cache_read_input_tokens":1000,"cache_creation_input_tokens":20,"output_tokens":7}}}"#;
        let events = parse_line(tool).unwrap();
        assert_eq!(
            events[1],
            ClaudeEvent::ToolUse {
                name: "Read".into(),
                detail: "src/a.ts".into()
            }
        );
        assert_eq!(events[2], ClaudeEvent::Context(1025));
        let rl = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
        assert!(matches!(parse_line(rl).unwrap()[0], ClaudeEvent::RateLimit(_)));
        let res = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.001095,"duration_ms":1794,"num_turns":1,"result":"done","session_id":"s-1","usage":{"input_tokens":645,"cache_read_input_tokens":100,"output_tokens":90},"permission_denials":[{"tool_name":"Bash","tool_input":{"command":"npx jest\nmore"}}]}"#;
        let ClaudeEvent::Result(r) = &parse_line(res).unwrap()[0] else {
            panic!("not a result")
        };
        assert_eq!(r.text, "done");
        assert_eq!(r.session_id, "s-1");
        assert_eq!((r.tokens_in, r.tokens_out, r.num_turns), (745, 90, 1));
        assert_eq!(r.denials, ["Bash: npx jest"]);
        assert!(!r.over_budget());
        assert!((r.cost_usd - 0.001095).abs() < 1e-12);
        assert!(parse_line("not json").is_err());
    }

    #[test]
    fn code_steps_resume_accept_edits_and_refuse_git_writes() {
        let args = code_args(
            Some("s-1"),
            5.0,
            &["npm run lint *".into(), "Bash(npx tsc *)".into()],
        );
        assert!(args.windows(2).any(|w| w[0] == "--resume" && w[1] == "s-1"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--allowedTools" && w[1] == "Bash(npm run lint *) Bash(npx tsc *)")
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--disallowedTools" && w[1].contains("git push"))
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--permission-mode" && w[1] == "acceptEdits")
        );
        let fresh = code_args(None, 2.0, &[]);
        assert!(!fresh.contains(&"--resume".to_owned()));
        assert!(!fresh.contains(&"--allowedTools".to_owned()));
        assert!(
            fresh
                .windows(2)
                .any(|w| w[0] == "--max-budget-usd" && w[1] == "2")
        );
        let analyze = analyze_args(1.5);
        assert!(analyze.contains(&"--restricted".to_owned()));
        assert!(
            analyze
                .windows(2)
                .any(|w| w[0] == "--tools" && w[1] == "Read,Grep,Glob")
        );
        assert!(
            analyze
                .windows(2)
                .any(|w| w[0] == "--max-budget-usd" && w[1] == "1.5")
        );
    }

    #[test]
    fn short_keeps_first_line_and_cuts_on_char_boundary() {
        assert_eq!(short("abc\ndef", 10), "abc");
        assert_eq!(short("ááááá", 3), "áá…");
    }

    #[test]
    fn review_local_passes_task_base_and_blocks_skill() {
        let args = review_local_args("origin/master", 3.0);
        let i = args.iter().position(|a| a == "--append-system-prompt").unwrap();
        assert!(args[i + 1].contains("origin/master"));
        // findings in Vietnamese and the docs/test judgement, asked in the same turn
        assert!(args[i + 1].contains("in Vietnamese") && args[i + 1].contains("`notes`"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--disallowedTools" && w[1] == "Skill")
        );
        assert!(!args.iter().any(|a| a.contains("git push")));
    }

    /// Measured on 2026-11-13: without these the child loads every tool and every MCP server.
    #[test]
    fn headless_steps_limit_tools_mcp_and_budget() {
        for args in [
            review_local_args("development", 3.0),
            review_args(3.0),
            push_summary_args(None, 1.0),
            push_summary_default_args(1.0),
            analyze_args(1.5),
            code_args(None, 5.0, &[]),
        ] {
            assert!(args.contains(&"--strict-mcp-config".to_owned()), "{args:?}");
            assert!(args.windows(2).any(|w| w[0] == "--tools" && !w[1].is_empty()));
            assert!(args.contains(&"--max-budget-usd".to_owned()));
            assert_eq!(args[0], "-p");
            assert_eq!(args.last().map(String::as_str), Some("--verbose"));
        }
        let push = push_summary_args(None, 1.0);
        assert!(push.windows(2).any(|w| w[0] == "--tools" && w[1] == "Bash"));
        assert_eq!(push[1], "/push-summary");
    }

    #[test]
    fn model_and_effort_flags_only_when_picked() {
        let plain = review_args(3.0);
        assert_eq!(with_model(plain.clone(), &ModelChoice::default()), plain);
        let args = with_model(
            review_args(3.0),
            &ModelChoice::new(Some("claude-sonnet-5-5"), Some("high")),
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--model" && w[1] == "claude-sonnet-5-5")
        );
        assert!(args.windows(2).any(|w| w[0] == "--effort" && w[1] == "high"));
        assert_eq!(args.last().map(String::as_str), Some("--verbose"));
        let effort_only = with_model(
            push_summary_default_args(1.0),
            &ModelChoice::new(None, Some("low")),
        );
        assert!(!effort_only.contains(&"--model".to_owned()));
        assert_eq!(effort_only.iter().filter(|a| *a == "--effort").count(), 1);
        assert!(ModelChoice::new(Some(" "), None).model.is_none());
    }

    #[test]
    fn probe_is_minimal() {
        let args = quota_probe_args("haiku");
        assert!(args.windows(2).any(|w| w[0] == "--tools" && w[1].is_empty()));
        assert!(args.contains(&"--no-session-persistence".to_owned()));
        assert!(
            push_summary_args(Some(456), 1.0)
                .iter()
                .any(|a| a.contains("#456"))
        );
    }
}
