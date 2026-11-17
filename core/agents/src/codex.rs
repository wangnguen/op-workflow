//! Codex CLI headless (`codex exec --json`). Flags verified with codex-cli 0.169.0 on
//! 2026-11-13 (PLAN.md §9), except the two `memories.*` overrides, which are not tried yet.

/// JSON Schema for `--output-schema`. OpenAI structured output needs every property listed in
/// `required` and `additionalProperties: false`; an optional value is typed as `[T, "null"]`.
pub const FINDINGS_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "required": ["findings"],
  "properties": {
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["severity", "file", "line", "title", "detail", "suggestion"],
        "properties": {
          "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] },
          "file": { "type": "string" },
          "line": { "type": ["integer", "null"] },
          "title": { "type": "string" },
          "detail": { "type": "string" },
          "suggestion": { "type": "string" }
        }
      }
    }
  }
}"#;

fn strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

/// `exec --sandbox <sandbox> --json` plus memories off, so Codex does not read
/// `~/.codex/memories`. The prompt goes through stdin (`-`, added by each builder last).
fn exec(sandbox: &str) -> Vec<String> {
    strings(&[
        "exec",
        "--sandbox",
        sandbox,
        "--json",
        "-c",
        "memories.use_memories=false",
        "-c",
        "memories.generate_memories=false",
    ])
}

/// Read-only review (Review chéo, and Tự review / Review chốt run by Codex). The final JSON
/// lands in `output_file`. The session is kept (no `--ephemeral`) so the Chat tab can offer
/// `codex resume <thread id>`.
pub fn cross_review_args(schema_file: &str, output_file: &str) -> Vec<String> {
    let mut args = exec("read-only");
    args.extend(strings(&["--output-schema", schema_file, "-o", output_file, "-"]));
    args
}

/// [2] Phân tích by Codex: read only; the plan is its last message.
pub fn analyze_args() -> Vec<String> {
    let mut args = exec("read-only");
    args.push("-".into());
    args
}

/// Viết code / Sửa lỗi / fixing what Kiểm tra found, and the harness: Codex may write in the
/// repo and in `dirs` (`--add-dir`). `resume` goes on in an earlier Codex session (the analysis,
/// or the last code step); the `exec` options stay on the parent command. Codex has no allow
/// list: its sandbox decides what a command may do, and the app's git guard and HEAD check stop
/// commits and pushes.
pub fn code_args(resume: Option<&str>, dirs: &[String]) -> Vec<String> {
    let mut args = exec("workspace-write");
    for d in dirs.iter().filter(|d| !d.trim().is_empty()) {
        args.extend(["--add-dir".to_owned(), d.clone()]);
    }
    args.push("-".into());
    match resume.filter(|id| !id.is_empty()) {
        Some(id) => with_resume(args, id),
        None => args,
    }
}

/// [12] Push summary by Codex: read only, nothing kept (like Claude's
/// `--no-session-persistence`); the answer is its last message.
pub fn push_args() -> Vec<String> {
    let mut args = exec("read-only");
    args.extend(strings(&["--ephemeral", "-"]));
    args
}

/// Go on in session `id`: `… resume <id> -` after the `exec` options.
pub fn with_resume(mut args: Vec<String>, id: &str) -> Vec<String> {
    if args.last().is_some_and(|a| a == "-") {
        args.pop();
    }
    args.extend(["resume".to_owned(), id.to_owned(), "-".to_owned()]);
    args
}

/// Flags that stand for a permission mode of their own (no `--sandbox` with them).
const MODE_FLAGS: [&str; 2] = ["approve-for-me", "dangerously-bypass-approvals-and-sandbox"];

/// Permission modes as `codex exec --help` lists them: the `--sandbox` values, then
/// `approve-for-me` / `dangerously-bypass-approvals-and-sandbox` when this version has the flag.
pub fn permission_modes(help: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(at) = help.find("--sandbox <") {
        let rest = &help[at..];
        // the flag's own text ends where the next flag starts
        let end = rest[1..].find("\n  -").map_or(rest.len(), |e| e + 1);
        if let Some(open) = rest[..end].find("[possible values:") {
            let list = &rest[open + "[possible values:".len()..end];
            let list = &list[..list.find(']').unwrap_or(list.len())];
            out.extend(
                list.split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
                    .map(str::to_owned),
            );
        }
    }
    for flag in MODE_FLAGS {
        if help.contains(&format!("--{flag}")) {
            out.push(flag.to_owned());
        }
    }
    out
}

/// Use the permission mode the user picked instead of the app's sandbox: a sandbox value
/// replaces `--sandbox <x>`; a mode flag replaces it with `--<flag>`.
pub fn with_mode(mut args: Vec<String>, mode: Option<&str>) -> Vec<String> {
    let Some(mode) = mode.map(str::trim).filter(|m| !m.is_empty()) else {
        return args;
    };
    let at = match args.iter().position(|a| a == "--sandbox") {
        Some(i) => {
            args.drain(i..(i + 2).min(args.len()));
            i
        }
        None => usize::from(args.first().is_some_and(|a| a == "exec")),
    };
    let flags = if MODE_FLAGS.contains(&mode) {
        vec![format!("--{mode}")]
    } else {
        vec!["--sandbox".to_owned(), mode.to_owned()]
    };
    args.splice(at..at, flags);
    args
}

/// Add `-m <model>` and `-c model_reasoning_effort="<effort>"` right after `exec` when the user
/// picked them for this step; otherwise Codex uses `~/.codex/config.toml`.
pub fn with_model(mut args: Vec<String>, choice: &crate::ModelChoice) -> Vec<String> {
    let at = usize::from(args.first().is_some_and(|a| a == "exec"));
    let mut flags = Vec::new();
    if let Some(m) = &choice.model {
        flags.extend(["-m".to_owned(), m.clone()]);
    }
    if let Some(e) = &choice.effort {
        flags.extend(["-c".to_owned(), format!("model_reasoning_effort=\"{e}\"")]);
    }
    args.splice(at..at, flags);
    args
}

/// Models named in `~/.codex/config.toml` (top level and profiles), for the Settings list.
pub fn configured_models(toml: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in toml.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "model" {
            continue;
        }
        let v = value
            .split('#')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches(['"', '\'']);
        if !v.is_empty() && !out.iter().any(|m| m == v) {
            out.push(v.to_owned());
        }
    }
    out
}

/// Events the app cares about from one `codex exec --json` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexEvent {
    Started {
        thread_id: String,
    },
    /// A shell command Codex runs (shortened).
    Command(String),
    /// A message from the agent (the last one is the answer).
    Message(String),
    /// Tokens of one turn.
    Usage {
        input: u64,
        cached: u64,
        output: u64,
    },
    /// `turn.failed` / `error`: what went wrong.
    Failed(String),
    Other(String),
}

pub fn parse_line(line: &str) -> crate::AgentsResult<CodexEvent> {
    use serde_json::Value;
    let v: Value = serde_json::from_str(line).map_err(|e| crate::AgentsError::BadLine(e.to_string()))?;
    let str_at = |p: &str| {
        v.pointer(p)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let num_at = |p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or_default();
    let kind = str_at("/type");
    Ok(match kind.as_str() {
        "thread.started" => CodexEvent::Started {
            thread_id: str_at("/thread_id"),
        },
        "item.started" | "item.completed" => match str_at("/item/type").as_str() {
            "command_execution" if kind == "item.started" => CodexEvent::Command(crate::claude::short(
                &str_at("/item/command"),
                crate::claude::TOOL_DETAIL_MAX,
            )),
            "agent_message" if kind == "item.completed" => CodexEvent::Message(str_at("/item/text")),
            other => CodexEvent::Other(format!("{kind}:{other}")),
        },
        "turn.completed" => CodexEvent::Usage {
            input: num_at("/usage/input_tokens"),
            cached: num_at("/usage/cached_input_tokens"),
            output: num_at("/usage/output_tokens"),
        },
        "turn.failed" => CodexEvent::Failed(str_at("/error/message")),
        "error" => CodexEvent::Failed(str_at("/message")),
        _ => CodexEvent::Other(kind),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exec_json_lines() {
        assert_eq!(
            parse_line(r#"{"type":"thread.started","thread_id":"t-1"}"#).unwrap(),
            CodexEvent::Started {
                thread_id: "t-1".into()
            }
        );
        assert_eq!(
            parse_line(r#"{"type":"item.started","item":{"id":"i","type":"command_execution","command":"git diff origin/main","status":"in_progress"}}"#).unwrap(),
            CodexEvent::Command("git diff origin/main".into())
        );
        assert_eq!(
            parse_line(r#"{"type":"item.completed","item":{"id":"i","type":"agent_message","text":"{\"findings\":[]}"}}"#).unwrap(),
            CodexEvent::Message("{\"findings\":[]}".into())
        );
        assert_eq!(
            parse_line(r#"{"type":"turn.completed","usage":{"input_tokens":393000,"cached_input_tokens":325000,"output_tokens":6200}}"#).unwrap(),
            CodexEvent::Usage {
                input: 393_000,
                cached: 325_000,
                output: 6200
            }
        );
        assert_eq!(
            parse_line(r#"{"type":"turn.failed","error":{"message":"401 Unauthorized"}}"#).unwrap(),
            CodexEvent::Failed("401 Unauthorized".into())
        );
    }

    #[test]
    fn cross_review_is_read_only_kept_and_reads_stdin() {
        let args = cross_review_args("s.json", "out.json");
        assert_eq!(args[0], "exec");
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--sandbox" && w[1] == "read-only")
        );
        assert!(!args.contains(&"--ephemeral".to_owned()));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-c" && w[1] == "memories.use_memories=false")
        );
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn code_steps_write_and_resume_in_their_session() {
        let fresh = code_args(None, &["C:/scratch".into(), " ".into()]);
        assert!(
            fresh
                .windows(2)
                .any(|w| w[0] == "--sandbox" && w[1] == "workspace-write")
        );
        assert!(
            fresh
                .windows(2)
                .any(|w| w[0] == "--add-dir" && w[1] == "C:/scratch")
        );
        assert_eq!(fresh.iter().filter(|a| *a == "--add-dir").count(), 1);
        assert_eq!(fresh.last().map(String::as_str), Some("-"));
        let resumed = code_args(Some("t-9"), &[]);
        assert_eq!(&resumed[resumed.len() - 3..], ["resume", "t-9", "-"]);
        assert_eq!(resumed.iter().filter(|a| *a == "-").count(), 1);
        assert!(analyze_args().windows(2).any(|w| w[1] == "read-only"));
        let push = push_args();
        assert!(push.contains(&"--ephemeral".to_owned()));
        assert_eq!(push.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn permission_modes_come_from_the_help_and_replace_the_sandbox() {
        let help = "  -s, --sandbox <SANDBOX_MODE>\n          Select the sandbox policy\n          \n          [possible values: read-only, workspace-write, danger-full-access]\n\n      --approve-for-me\n          Route approval requests\n\n      --dangerously-bypass-approvals-and-sandbox\n          Skip all\n";
        assert_eq!(
            permission_modes(help),
            [
                "read-only",
                "workspace-write",
                "danger-full-access",
                "approve-for-me",
                "dangerously-bypass-approvals-and-sandbox"
            ]
        );
        assert!(permission_modes("  -m, --model <MODEL>\n").is_empty());
        let code = code_args(None, &[]);
        assert_eq!(with_mode(code.clone(), None), code);
        let full = with_mode(code.clone(), Some("danger-full-access"));
        assert!(
            full.windows(2)
                .any(|w| w[0] == "--sandbox" && w[1] == "danger-full-access")
        );
        assert_eq!(full.iter().filter(|a| *a == "--sandbox").count(), 1);
        let bypass = with_mode(code, Some("dangerously-bypass-approvals-and-sandbox"));
        assert!(!bypass.contains(&"--sandbox".to_owned()));
        assert_eq!(bypass[1], "--dangerously-bypass-approvals-and-sandbox");
    }

    #[test]
    fn model_flags_go_after_exec_and_models_come_from_config() {
        let plain = cross_review_args("s.json", "o.json");
        assert_eq!(with_model(plain.clone(), &crate::ModelChoice::default()), plain);
        let args = with_model(plain, &crate::ModelChoice::new(Some("gpt-x"), Some("high")));
        assert_eq!(
            &args[..5],
            ["exec", "-m", "gpt-x", "-c", "model_reasoning_effort=\"high\""]
        );
        assert_eq!(args.last().map(String::as_str), Some("-"));
        let toml = "model = \"gpt-a\" # main\nmodel_reasoning_effort = \"xhigh\"\n[profiles.fast]\nmodel = 'gpt-b'\n[x]\nmodel = \"gpt-a\"\n";
        assert_eq!(configured_models(toml), ["gpt-a", "gpt-b"]);
    }

    #[test]
    fn schema_is_strict() {
        let v: serde_json::Value = serde_json::from_str(FINDINGS_SCHEMA).unwrap();
        let item = &v["properties"]["findings"]["items"];
        assert_eq!(item["additionalProperties"], false);
        assert_eq!(item["required"].as_array().unwrap().len(), 6);
    }
}
