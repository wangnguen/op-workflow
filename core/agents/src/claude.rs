//! Claude Code headless (`claude -p --output-format stream-json --verbose`).

use serde_json::Value;

use crate::{AgentsError, AgentsResult};

/// git subcommands an AI process may never run (defence in depth on top of the env guard).
pub const DISALLOWED_GIT: &str = "Bash(git push *) Bash(git commit *) Bash(git merge *) Bash(git rebase *) \
                                  Bash(git reset *) Bash(git clean *) Bash(git tag *) Bash(gh pr *)";

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
    },
    AssistantText,
    /// Raw `rate_limit_event`; `opw-quota` turns it into a QuotaSnapshot.
    RateLimit(Value),
    Result {
        is_error: bool,
        cost_usd: f64,
        duration_ms: u64,
        num_turns: u64,
    },
    Other(String),
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
            blocks
                .iter()
                .map(|b| match b.get("type").and_then(Value::as_str) {
                    Some("tool_use") => ClaudeEvent::ToolUse {
                        name: b
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    },
                    _ => ClaudeEvent::AssistantText,
                })
                .collect()
        }
        "rate_limit_event" => vec![ClaudeEvent::RateLimit(v)],
        "result" => vec![ClaudeEvent::Result {
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            cost_usd: v
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or_default(),
            num_turns: v.get("num_turns").and_then(Value::as_u64).unwrap_or_default(),
        }],
        other => vec![ClaudeEvent::Other(other.to_owned())],
    })
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

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

/// `/review-local` run headless. The repo command hard-codes `development`, so the real base
/// of the task is passed in; `Skill` is blocked so it cannot chain into `/push-summary`.
pub fn review_local_args(base: &str) -> Vec<String> {
    let prompt = format!(
        "Non-interactive run. The base branch for this task is {base}: wherever the command uses \
         development, use {base} instead. End with one json block: \
         {{findings:[{{severity,file,line,title,detail,suggestion}}]}}"
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
        "Bash(git diff *) Bash(git status *) Bash(git log *)",
        "--max-budget-usd",
        "3",
        "--append-system-prompt",
    ]);
    args.push(prompt);
    args.extend(strings(&["--output-format", "stream-json", "--verbose"]));
    args
}

/// `/push-summary` with the issue number the user confirmed on the push screen.
pub fn push_summary_args(issue: Option<u64>) -> Vec<String> {
    let note = match issue {
        Some(n) => format!("Issue number: #{n} (already provided, do not ask)"),
        None => "There is no issue number for this change (do not ask)".to_owned(),
    };
    let mut args = strings(&[
        "-p",
        "/push-summary",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--effort",
        "low",
        // Without --tools the child loads every tool on the machine (~39k extra tokens, §9).
        "--tools",
        "Bash",
        "--permission-prompts",
        "none",
        "--disallowedTools",
        "Skill",
        "--allowedTools",
        "Bash(git diff *) Bash(git log *)",
        "--max-budget-usd",
        "1",
        "--append-system-prompt",
    ]);
    args.push(note);
    args.extend(strings(&["--output-format", "stream-json", "--verbose"]));
    args
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
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"Read","input":{}}]}}"#;
        assert_eq!(
            parse_line(tool).unwrap()[1],
            ClaudeEvent::ToolUse { name: "Read".into() }
        );
        let rl = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
        assert!(matches!(parse_line(rl).unwrap()[0], ClaudeEvent::RateLimit(_)));
        let res = r#"{"type":"result","is_error":false,"total_cost_usd":0.001095,"duration_ms":1794,"num_turns":1}"#;
        assert_eq!(
            parse_line(res).unwrap(),
            vec![ClaudeEvent::Result {
                is_error: false,
                cost_usd: 0.001095,
                duration_ms: 1794,
                num_turns: 1
            }]
        );
        assert!(parse_line("not json").is_err());
    }

    #[test]
    fn review_local_passes_task_base_and_blocks_skill() {
        let args = review_local_args("origin/master");
        let i = args.iter().position(|a| a == "--append-system-prompt").unwrap();
        assert!(args[i + 1].contains("origin/master"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--disallowedTools" && w[1] == "Skill")
        );
        assert!(!args.iter().any(|a| a.contains("git push")));
    }

    /// Measured on 2026-11-13: without these the child loads every tool and every MCP server.
    #[test]
    fn headless_steps_limit_tools_mcp_and_budget() {
        for args in [review_local_args("development"), push_summary_args(None)] {
            assert!(args.contains(&"--strict-mcp-config".to_owned()));
            assert!(args.windows(2).any(|w| w[0] == "--tools" && !w[1].is_empty()));
            assert!(args.contains(&"--max-budget-usd".to_owned()));
        }
        let push = push_summary_args(None);
        assert!(push.windows(2).any(|w| w[0] == "--tools" && w[1] == "Bash"));
    }

    #[test]
    fn probe_is_minimal() {
        let args = quota_probe_args("haiku");
        assert!(args.windows(2).any(|w| w[0] == "--tools" && w[1].is_empty()));
        assert!(args.contains(&"--no-session-persistence".to_owned()));
        assert!(push_summary_args(Some(456)).iter().any(|a| a.contains("#456")));
    }
}
