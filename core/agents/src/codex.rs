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

/// Read-only cross-review. The prompt goes through stdin (`-`); the final JSON lands in
/// `output_file`. Memories are off so Codex does not read `~/.codex/memories`.
pub fn cross_review_args(schema_file: &str, output_file: &str) -> Vec<String> {
    [
        "exec",
        "--sandbox",
        "read-only",
        "--ephemeral",
        "--json",
        "--output-schema",
        schema_file,
        "-c",
        "memories.use_memories=false",
        "-c",
        "memories.generate_memories=false",
        "-o",
        output_file,
        "-",
    ]
    .iter()
    .map(|a| (*a).to_owned())
    .collect()
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
    fn cross_review_is_read_only_ephemeral_and_reads_stdin() {
        let args = cross_review_args("s.json", "out.json");
        assert_eq!(args[0], "exec");
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--sandbox" && w[1] == "read-only")
        );
        assert!(args.contains(&"--ephemeral".to_owned()));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-c" && w[1] == "memories.use_memories=false")
        );
        assert_eq!(args.last().map(String::as_str), Some("-"));
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
